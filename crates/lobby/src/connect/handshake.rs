use flo_net::connect::*;
use flo_net::packet::*;
use flo_net::stream::FloStream;

use crate::error::*;
use crate::game::Game;
use crate::player::token::validate_player_token;

pub async fn handle_handshake(stream: &mut FloStream) -> Result<ConnectState> {
  let req: PacketConnectLobby = stream.recv().await?;
  let client_version = req.connect_version.extract()?;

  tracing::debug!("client version = {}", client_version);
  let token = validate_player_token(&req.token)?;

  tracing::debug!(token.player_id);

  Ok(ConnectState {
    player_id: token.player_id,
    joined_game: None,
  })
}

#[derive(Debug)]
pub struct ConnectState {
  pub player_id: i32,
  pub joined_game: Option<Game>,
}
