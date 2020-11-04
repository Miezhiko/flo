use futures::future::abortable;
use s2_grpc_utils::{S2ProtoPack, S2ProtoUnpack};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::stream::StreamExt;
use tokio::sync::Notify;
use tokio::time::delay_for;

use flo_net::connect;
use flo_net::listener::FloListener;
use flo_net::packet::FloPacket;
use flo_net::packet::OptionalFieldExt;
use flo_net::proto;
use flo_net::stream::FloStream;
use flo_net::time::StopWatch;

use crate::error::*;
use crate::state::{ActorMapExt, ControllerStateRef};

mod handshake;
mod sender;
use crate::game::messages::{ResolveGamePlayerPingBroadcastTargets, UpdateSlot};
use crate::game::state::node::SelectNode;
use crate::game::state::player::GetGamePlayers;
use crate::game::state::start::{StartGameCheck, StartGamePlayerAck};
use crate::game::SlotSettings;
use crate::node::messages::ListNode;
use crate::player::state::conn::{Connect, Disconnect};
use crate::player::state::ping::{GetPlayersPingSnapshot, UpdatePing};
use flo_types::ping::PingStats;
pub use sender::{PlayerReceiver, PlayerSender, PlayerSenderMessage};

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn serve(state: ControllerStateRef) -> Result<()> {
  state
    .db
    .exec(|conn| crate::game::db::reset_instance_state(conn))
    .await?;

  let mut listener = FloListener::bind_v4(flo_constants::CONTROLLER_SOCKET_PORT).await?;
  tracing::info!("listening on port {}", listener.port());

  while let Some(mut stream) = listener.incoming().try_next().await? {
    let state = state.clone();
    tokio::spawn(async move {
      tracing::debug!("connected: {}", stream.peer_addr()?);

      let accepted = match handshake::handle_handshake(&mut stream).await {
        Ok(accepted) => accepted,
        Err(e) => {
          tracing::debug!("dropping: handshake error: {}", e);
          return Ok(());
        }
      };

      let player_id = accepted.player_id;
      tracing::debug!("accepted: player_id = {}", player_id);

      if accepted.client_version < flo_constants::MIN_FLO_VERSION {
        stream
          .send(proto::flo_connect::PacketClientConnectReject {
            lobby_version: Some(From::from(crate::version::FLO_LOBBY_VERSION)),
            reason: proto::flo_connect::ClientConnectRejectReason::ClientVersionTooOld.into(),
          })
          .await?;
        stream.flush().await?;
        return Ok(());
      }

      let receiver = {
        let (sender, r) = PlayerSender::new(player_id);
        state.players.send(Connect { sender }).await?;
        r
      };

      if let Err(err) = handle_stream(state.clone(), player_id, stream, receiver).await {
        tracing::debug!("stream error: {}", err);
      }

      state.players.send(Disconnect { player_id }).await?;
      tracing::debug!("exiting: player_id = {}", player_id);
      Ok::<_, crate::error::Error>(())
    });
  }

  tracing::info!("exiting");

  Ok(())
}

#[tracing::instrument(target = "player_stream", skip(state, stream))]
async fn handle_stream(
  state: ControllerStateRef,
  player_id: i32,
  mut stream: FloStream,
  mut receiver: PlayerReceiver,
) -> Result<()> {
  send_initial_state(state.clone(), &mut stream, player_id).await?;

  let stop_watch = StopWatch::new();
  let ping_timeout_notify = Arc::new(Notify::new());
  let mut ping_timeout_abort = None;

  loop {
    let mut next_ping = delay_for(PING_INTERVAL);

    tokio::select! {
      _ = &mut next_ping => {
        let notify = ping_timeout_notify.clone();

        stream.send(proto::flo_common::PacketPing {
          ms: stop_watch.elapsed_ms()
        }).await?;
        let (set_ping_timeout, abort) = abortable(async move {
          delay_for(PING_TIMEOUT).await;
          notify.notify();
        });
        ping_timeout_abort = Some(abort);
        tokio::spawn(set_ping_timeout);
      }
      _ = ping_timeout_notify.notified() => {
          tracing::error!("heartbeat timeout");
          break;
      }
      next = receiver.recv() => {
        if let Some(msg) = next {
          match msg {
            PlayerSenderMessage::Frame(frame) => {
              if let Err(e) = stream.send_frame(frame).await {
                tracing::debug!("send error: {}", e);
                break;
              }
            }
            PlayerSenderMessage::Disconnect(reason) => {
              use flo_net::proto::flo_connect::PacketClientDisconnect;
              if let Err(e) = stream.send(PacketClientDisconnect {
                reason: reason.into()
              }).await {
                tracing::debug!("send error: {}", e);
              }
              break;
            }
          }
        } else {
          tracing::debug!("sender dropped");
          break;
        }
      }
      incoming = stream.recv_frame() => {
        if let Some(abort) = ping_timeout_abort.take() {
          abort.abort();
        }

        let frame = incoming?;

        flo_net::try_flo_packet! {
          frame => {
            packet: proto::flo_common::PacketPong => {
              //TODO: save ping and display on UI
              // tracing::debug!("pong, latency = {}", stop_watch.elapsed_ms().saturating_sub(packet.ms));
            }
            packet: proto::flo_connect::PacketGameSlotUpdateRequest => {
              handle_game_slot_update_request(state.clone(), player_id, packet).await?;
            }
            _packet: proto::flo_connect::PacketListNodesRequest => {
              handle_list_nodes_request(state.clone(), player_id).await?;
            }
            packet: proto::flo_connect::PacketPlayerPingMapUpdateRequest => {
              handle_player_ping_map_update_request(state.clone(), player_id, packet).await?;
            }
            packet: proto::flo_connect::PacketGamePlayerPingMapSnapshotRequest => {
              handle_game_player_ping_map_snapshot_request(state.clone(), player_id, packet.game_id).await?;
            }
            packet: proto::flo_connect::PacketGameSelectNodeRequest => {
              handle_game_select_node_request(state.clone(), player_id, packet).await?;
            }
            packet: flo_net::proto::flo_connect::PacketGameStartRequest => {
              handle_game_start_request(state.clone(), player_id, packet).await?;
            }
            packet: flo_net::proto::flo_connect::PacketGameStartPlayerClientInfoRequest => {
              handle_game_start_player_client_info_request(state.clone(), player_id, packet).await?;
            }
          }
        }
      }
    }
  }

  Ok(())
}

async fn send_initial_state(
  state: ControllerStateRef,
  stream: &mut FloStream,
  player_id: i32,
) -> Result<()> {
  let (player, active_slots) = state
    .db
    .exec(move |conn| -> Result<_> {
      Ok((
        crate::player::db::get_ref(conn, player_id)?,
        crate::game::db::get_player_active_slots(conn, player_id)?,
      ))
    })
    .await?;

  let game_id = active_slots.last().map(|s| s.game_id);

  let mut frames = vec![connect::PacketClientConnectAccept {
    lobby_version: Some(From::from(crate::version::FLO_LOBBY_VERSION)),
    session: Some({
      use proto::flo_connect::*;
      Session {
        player: player.pack()?,
        status: if game_id.is_some() {
          PlayerStatus::InGame.into()
        } else {
          PlayerStatus::Idle.into()
        },
        game_id: game_id.clone(),
      }
    }),
    nodes: state.nodes.send(ListNode).await?.pack()?,
  }
  .encode_as_frame()?];

  if let Some(game_id) = game_id {
    let (game, node_player_token) = state
      .db
      .exec(move |conn| crate::game::db::get_full_and_node_token(conn, game_id, player_id))
      .await?;

    let node_id = game.node.as_ref().map(|node| node.id);

    let game = game.pack()?;

    let frame = connect::PacketGameInfo { game: Some(game) }.encode_as_frame()?;
    frames.push(frame);

    if let Some(player_token) = node_player_token {
      let frame = connect::PacketGamePlayerToken {
        node_id: node_id.ok_or_else(|| Error::GameNodeNotSelected)?,
        game_id,
        player_id,
        player_token: player_token.to_vec(),
      }
      .encode_as_frame()?;
      frames.push(frame);
    }
  }

  stream.send_frames(frames).await?;
  Ok(())
}

async fn handle_game_slot_update_request(
  state: ControllerStateRef,
  player_id: i32,
  packet: proto::flo_connect::PacketGameSlotUpdateRequest,
) -> Result<()> {
  state
    .games
    .send_to(
      packet.game_id,
      UpdateSlot {
        player_id,
        slot_index: packet.slot_index,
        settings: SlotSettings::unpack(packet.slot_settings.extract()?)?,
      },
    )
    .await?;
  Ok(())
}

async fn handle_list_nodes_request(state: ControllerStateRef, player_id: i32) -> Result<()> {
  let nodes = state.nodes.send(ListNode).await?;
  let packet = proto::flo_connect::PacketListNodes {
    nodes: nodes.pack()?,
  };
  state
    .player_packet_sender
    .send(player_id, packet.encode_as_frame()?)
    .await?;
  Ok(())
}

async fn handle_player_ping_map_update_request(
  state: ControllerStateRef,
  player_id: i32,
  packet: proto::flo_connect::PacketPlayerPingMapUpdateRequest,
) -> Result<()> {
  use std::collections::BTreeMap;
  let ping_map: BTreeMap<_, _> = packet
    .ping_map
    .clone()
    .into_iter()
    .map(|(k, v)| Ok((k, PingStats::unpack(v)?)))
    .collect::<Result<Vec<_>>>()?
    .into_iter()
    .collect();
  let mut node_ids: Vec<_> = ping_map.keys().cloned().collect();

  state
    .players
    .send(UpdatePing {
      player_id,
      ping_map,
    })
    .await?;

  node_ids.sort();
  node_ids.dedup();

  let targets = state
    .games
    .send(ResolveGamePlayerPingBroadcastTargets {
      player_id,
      node_ids,
    })
    .await??;

  state
    .player_packet_sender
    .broadcast(
      targets,
      proto::flo_connect::PacketPlayerPingMapUpdate {
        player_id,
        ping_map: packet.ping_map,
      }
      .encode_as_frame()?,
    )
    .await?;

  Ok(())
}

async fn handle_game_player_ping_map_snapshot_request(
  state: ControllerStateRef,
  player_id: i32,
  game_id: i32,
) -> Result<()> {
  use flo_net::proto::flo_connect::*;

  let players = state.games.send_to(game_id, GetGamePlayers).await?;
  let snapshot = state
    .players
    .send(GetPlayersPingSnapshot { players })
    .await?;

  let mut node_ping_map = HashMap::<i32, NodePingMap>::new();
  for (player_id, node_map) in snapshot.map {
    for (node_id, ping) in node_map {
      let item = node_ping_map
        .entry(node_id)
        .or_insert_with(|| Default::default());
      item.player_ping_map.insert(player_id, ping.pack()?);
    }
  }

  state
    .player_packet_sender
    .send(
      player_id,
      PacketGamePlayerPingMapSnapshot {
        game_id,
        node_ping_map,
      }
      .encode_as_frame()?,
    )
    .await?;

  Ok(())
}

async fn handle_game_select_node_request(
  state: ControllerStateRef,
  player_id: i32,
  packet: proto::flo_connect::PacketGameSelectNodeRequest,
) -> Result<()> {
  state
    .games
    .send_to(
      packet.game_id,
      SelectNode {
        node_id: packet.node_id,
        player_id,
      },
    )
    .await?;
  Ok(())
}

async fn handle_game_start_request(
  state: ControllerStateRef,
  player_id: i32,
  packet: proto::flo_connect::PacketGameStartRequest,
) -> Result<()> {
  state
    .games
    .send_to(packet.game_id, StartGameCheck { player_id })
    .await?;
  Ok(())
}

async fn handle_game_start_player_client_info_request(
  state: ControllerStateRef,
  player_id: i32,
  packet: proto::flo_connect::PacketGameStartPlayerClientInfoRequest,
) -> Result<()> {
  state
    .games
    .send_to(packet.game_id, StartGamePlayerAck::new(player_id, packet))
    .await?;
  Ok(())
}
