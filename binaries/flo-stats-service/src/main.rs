mod graphql;

use crate::graphql::{FloLiveSchema, MutationRoot, QueryRoot, SubscriptionRoot};
use async_graphql::http::{playground_source, GraphQLPlaygroundConfig};
use async_graphql::Schema;
use async_graphql_axum::{GraphQLRequest, GraphQLResponse, GraphQLSubscription};
use axum::headers::HeaderValue;
use axum::http::Method;
use axum::response::{self, IntoResponse};
use axum::routing::get;
use axum::{extract, AddExtensionLayer, Router, Server};
use flo_observer_edge::FloObserverEdge;
use tower_http::cors::{CorsLayer, Origin};

async fn graphql_handler(
  schema: extract::Extension<FloLiveSchema>,
  req: GraphQLRequest,
) -> GraphQLResponse {
  schema.execute(req.into_inner()).await.into()
}

async fn graphql_playground() -> impl IntoResponse {
  response::Html(playground_source(
    GraphQLPlaygroundConfig::new("/").subscription_endpoint("/ws"),
  ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  #[cfg(debug_assertions)]
  {
    dotenv::dotenv()?;
    flo_log_subscriber::init_env_override("flo_stats_service=debug,flo_observer_edge=debug");
  }

  #[cfg(not(debug_assertions))]
  {
    flo_log_subscriber::init();
  }

  let edge = FloObserverEdge::from_env().await?;

  let schema = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
    .data(edge.handle())
    .finish();

  tokio::spawn(async move {
    if let Err(err) = edge.serve().await {
      tracing::error!("stream server: {}", err);
    }
  });

  let app = Router::new()
    .route("/", get(graphql_playground).post(graphql_handler))
    .route("/ws", GraphQLSubscription::new(schema.clone()))
    .layer(AddExtensionLayer::new(schema))
    .layer({
      let allowed_list: [HeaderValue; 4] = [
        "http://localhost:3000".parse().unwrap(),
        "http://localhost:3558".parse().unwrap(),
        "https://w3flo.com".parse().unwrap(),
        "https://stats.w3flo.com".parse().unwrap(),
      ];
      CorsLayer::new()
        .allow_origin(Origin::list(allowed_list))
        .allow_methods(vec![Method::POST])
        .allow_headers(tower_http::cors::any())
    });

  let bind = format!("0.0.0.0:{}", flo_constants::OBSERVER_GRAPHQL_PORT);

  tracing::info!("running at {}", bind);

  Server::bind(&bind.parse().unwrap())
    .serve(app.into_make_service())
    .await?;
  Ok(())
}
