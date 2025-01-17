mod broadcast;
mod constants;
mod controller;
mod dispatcher;
mod env;
mod error;
pub mod game;
mod server;
mod services;
mod version;
mod archiver;

use crate::archiver::Archiver;
use crate::broadcast::BroadcastReceiver;
use dispatcher::{
  AddIterator, Dispatcher, GetGame, ListGames, SubscribeGameListUpdate, SubscribeGameUpdate,
};
use error::Result;
use flo_kinesis::{data_stream::DataStream, iterator::ShardIteratorType};
use flo_state::{Actor, Addr, Owner};
use game::event::{GameListUpdateEvent, GameUpdateEvent};
use game::snapshot::{GameSnapshot, GameSnapshotWithStats};
use server::StreamServer;
use services::Services;
use std::time::Duration;

pub struct FloObserverEdge {
  dispatcher: Owner<Dispatcher>,
  stream_server: StreamServer,
  archiver: Option<Archiver>,
}

impl FloObserverEdge {
  pub async fn from_env() -> Result<Self> {
    let mut services = Services::from_env();
    let archiver = if let Some((archiver, handle)) = Archiver::new()? {
      services.archiver = Some(handle);
      tracing::debug!("archiver enabled.");
      Some(archiver)
    } else {
      tracing::debug!("archiver disabled.");
      None
    };
    let dispatcher = Dispatcher::new(services).start();

    let data_stream = DataStream::from_env();
    let iter_type = ShardIteratorType::at_timestamp_backward(Duration::from_secs(
      crate::env::ENV.record_backscan_secs,
    ));

    tracing::debug!("creating iterator...");

    let iter = data_stream.into_iter(iter_type).await?;

    tracing::debug!("iterator created.");

    dispatcher.send(AddIterator(iter)).await?;

    tracing::debug!("iterator added.");

    let stream_server = StreamServer::new(dispatcher.addr()).await?;

    tracing::debug!(
      "server listening on {}",
      flo_constants::OBSERVER_SOCKET_PORT
    );

    Ok(Self {
      dispatcher,
      stream_server,
      archiver,
    })
  }

  pub async fn serve(self) -> Result<()> {
    if let Some(archiver) = self.archiver {
      tokio::pin! {
        let f1 = self.stream_server.serve();
        let f2 = archiver.serve();
      };
      let r = futures::future::select(
        f1, f2
      ).await;
      match r {
        futures::future::Either::Left((r, _)) => r,
        futures::future::Either::Right((_, f)) => f.await,
      }
    } else {
      self.stream_server.serve().await
    }
  }

  pub fn handle(&self) -> FloObserverEdgeHandle {
    FloObserverEdgeHandle(self.dispatcher.addr())
  }
}

#[derive(Clone)]
pub struct FloObserverEdgeHandle(Addr<Dispatcher>);

impl FloObserverEdgeHandle {
  pub async fn list_games(&self) -> Result<Vec<GameSnapshot>> {
    self.0.send(ListGames).await.map_err(Into::into)
  }

  pub async fn get_game(&self, game_id: i32) -> Result<GameSnapshot> {
    let game = self.0.send(GetGame { game_id }).await??;
    Ok(game)
  }

  pub async fn subscribe_game_list_updates(
    &self,
  ) -> Result<(Vec<GameSnapshot>, BroadcastReceiver<GameListUpdateEvent>)> {
    self.0.send(SubscribeGameListUpdate).await?
  }

  pub async fn subscribe_game_updates(
    &self,
    game_id: i32,
  ) -> Result<(GameSnapshotWithStats, BroadcastReceiver<GameUpdateEvent>)> {
    self.0.send(SubscribeGameUpdate { game_id }).await?
  }
}
