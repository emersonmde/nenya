use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use crate::sentinel::{ShouldThrottleRequest, ShouldThrottleResponse};
use nenya::RateLimiter;
use sentinel::sentinel_server::{Sentinel, SentinelServer};
use sentinel::{MetricData, Metrics};

pub mod sentinel {
    tonic::include_proto!("sentinel");
}

type MetricMap = HashMap<String, MetricData>;
type LockedMetricMap = Arc<RwLock<MetricMap>>;

#[derive(Debug, Default)]
pub struct SentinelService {
    segments: Arc<RwLock<HashMap<String, RateLimiter>>>,
    node_metrics: Arc<RwLock<HashMap<String, LockedMetricMap>>>,
}

#[tonic::async_trait]
impl Sentinel for SentinelService {
    async fn exchange_metrics(
        &self,
        request: Request<Metrics>,
    ) -> Result<Response<Metrics>, Status> {
        let node_metrics = request.into_inner();

        let node_metrics_guard = self.node_metrics.read().await;
        let node_metrics_value = node_metrics_guard.get(&node_metrics.source);

        if let Some(metrics_value_lock) = node_metrics_value {
            let mut metrics_value_guard = metrics_value_lock.write().await;
            *metrics_value_guard = node_metrics.segments;
        } else {
            drop(node_metrics_guard);
            let mut node_metrics_guard = self.node_metrics.write().await;
            node_metrics_guard.insert(
                node_metrics.source,
                Arc::new(RwLock::new(node_metrics.segments)),
            );
        }

        let segments = self.segments.read().await;
        let metric_segments: HashMap<String, MetricData> = segments
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    MetricData {
                        // TODO: Switch RateLimiter to f32
                        request_rate: v.request_rate() as f32,
                        accepted_request_rate: v.accepted_request_rate() as f32,
                    },
                )
            })
            .collect();

        return Ok(Response::new(Metrics {
            segments: metric_segments,
            // TODO: use local IP
            source: "foo".to_string(),
        }));
    }

    async fn should_throttle(
        &self,
        _request: Request<ShouldThrottleRequest>,
    ) -> Result<Response<ShouldThrottleResponse>, Status> {
        todo!()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:8080".parse()?;
    let sentinel = SentinelService::default();

    Server::builder()
        .add_service(SentinelServer::new(sentinel))
        .serve(addr)
        .await?;

    Ok(())
}
