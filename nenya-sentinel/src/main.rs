use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use nenya::pid_controller::PIDController;
use nenya::{RateLimiter, RateLimiterBuilder};
use sentinel::sentinel_server::{Sentinel, SentinelServer};
use sentinel::{MetricData, Metrics};

use crate::sentinel::{SegmentConfig, ShouldThrottleRequest, ShouldThrottleResponse};

pub mod sentinel {
    tonic::include_proto!("sentinel");
}

type SegmentMetrics = HashMap<String, MetricData>;
type LockedSegmentMetrics = Arc<RwLock<SegmentMetrics>>;

#[derive(Debug, Default)]
pub struct SentinelService {
    segments: Arc<RwLock<HashMap<String, RateLimiter<f32>>>>,
    node_metrics: Arc<RwLock<HashMap<String, LockedSegmentMetrics>>>,
    hostname: String,
    _default_segment_config: SegmentConfig,
}

impl SentinelService {
    pub fn new(
        hostname: String,
        peers: Vec<String>,
        segments: HashMap<String, SegmentConfig>,
        default_segment_config: SegmentConfig,
        pid_controller: PIDController<f32>,
    ) -> Self {
        let segment_limiters: HashMap<String, RateLimiter<f32>> = segments
            .iter()
            .map(|(segment_name, segment_config)| {
                let mut rate_limiter = RateLimiterBuilder::new(segment_config.target_tps);
                if let Some(min_tps) = segment_config.min_tps {
                    rate_limiter = rate_limiter.min_rate(min_tps);
                }
                if let Some(max_tps) = segment_config.max_tps {
                    rate_limiter = rate_limiter.max_rate(max_tps);
                }
                (
                    segment_name.clone(),
                    rate_limiter.pid_controller(pid_controller.clone()).build(),
                )
            })
            .collect();
        let node_metrics = peers
            .iter()
            .map(|node| (node.clone(), Arc::new(RwLock::new(HashMap::new()))))
            .collect();
        SentinelService {
            hostname,
            node_metrics: Arc::new(RwLock::new(node_metrics)),
            segments: Arc::new(RwLock::new(segment_limiters)),
            _default_segment_config: default_segment_config,
        }
    }
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
            .map(|(segment_id, segment_rate_limiter)| {
                (
                    segment_id.clone(),
                    MetricData {
                        request_rate: segment_rate_limiter.request_rate(),
                        accepted_request_rate: segment_rate_limiter.accepted_request_rate(),
                    },
                )
            })
            .collect();

        return Ok(Response::new(Metrics {
            segments: metric_segments,
            source: self.hostname.clone(),
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
    let hostname: String = hostname::get()?
        .into_string()
        .expect("Unable to get hostname");
    let peers = vec!["foo".to_string(), "bar".to_string()];
    let default_segment_config = SegmentConfig {
        target_tps: 100.0,
        min_tps: None,
        max_tps: None,
    };
    let pid_controller = PIDController::new_static_controller(100.0);
    let sentinel = SentinelService::new(
        hostname,
        peers,
        HashMap::default(),
        default_segment_config,
        pid_controller,
    );

    Server::builder()
        .add_service(SentinelServer::new(sentinel))
        .serve(addr)
        .await?;

    Ok(())
}
