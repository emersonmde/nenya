# Nenya Architecture

This document describes the architecture of Nenya, a distributed adaptive rate limiter using PID control and gossip-based coordination.

## Overview

Nenya consists of two components:

- **nenya**: Core Rust library providing adaptive rate limiting with PID control (no distributed features)
- **nenya-sentinel**: Standalone binary/sidecar that wraps nenya and adds distributed coordination via gossip protocol

## Design Goals

1. **Minimal configuration**: Zero-config with sensible defaults, configurable when needed
2. **Universal deployment**: Works in Docker, Kubernetes, traditional VMs, or bare metal
3. **Secure by default**: Requires cluster authentication, supports mTLS
4. **Horizontally scalable**: No upper bound on cluster size using gossip protocol
5. **Eventually consistent**: Accepts brief rate limit overage during network partitions, degrades gracefully
6. **Simple integration**: Applications just need to call a local HTTP endpoint

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ Application Container/Process (any language)                │
│                                                              │
│  if http_post("localhost:8080/should_throttle",            │
│                {"scope": "api-endpoint"}) {                 │
│      return 429;                                            │
│  }                                                          │
└───────────────────────┬──────────────────────────────────────┘
                        │ HTTP/JSON to localhost
                        ▼
┌─────────────────────────────────────────────────────────────┐
│ Nenya Sentinel (sidecar/daemon)                             │
│                                                              │
│ ┌────────────────────────────────────────────────────────┐  │
│ │ HTTP Server (axum) - binds to 127.0.0.1:8080          │  │
│ │                                                        │  │
│ │ Endpoints:                                            │  │
│ │  POST /should_throttle → throttle decision           │  │
│ │  GET  /health         → health check                 │  │
│ │  GET  /metrics        → Prometheus metrics           │  │
│ └────────────────┬───────────────────────────────────────┘  │
│                  │                                           │
│                  ▼                                           │
│ ┌────────────────────────────────────────────────────────┐  │
│ │ Rate Limit Manager                                     │  │
│ │                                                        │  │
│ │ HashMap<String, RateLimiter<f64>>                     │  │
│ │                                                        │  │
│ │ Scopes (auto-created on first use):                  │  │
│ │  "api-endpoint"    → RateLimiter + PIDController     │  │
│ │  "login"           → RateLimiter + PIDController     │  │
│ │  "api#key_abc123"  → RateLimiter + PIDController     │  │
│ │                                                        │  │
│ │ Each RateLimiter:                                     │  │
│ │  - Tracks local request rate (sliding window)        │  │
│ │  - Receives aggregated remote rates from gossip      │  │
│ │  - PID controller adjusts target based on total rate │  │
│ └────────────────┬───────────────────────────────────────┘  │
│                  │                                           │
│                  ▼                                           │
│ ┌────────────────────────────────────────────────────────┐  │
│ │ Scope Pattern Matcher                                  │  │
│ │                                                        │  │
│ │ Configuration (nenya.toml):                           │  │
│ │  [[rate_limits]]                                      │  │
│ │  pattern = "api#*"                                    │  │
│ │  target_rate = 100.0                                  │  │
│ │  min_rate = 50.0                                      │  │
│ │  max_rate = 200.0                                     │  │
│ │                                                        │  │
│ │  [[rate_limits]]                                      │  │
│ │  pattern = "api#premium_*"                           │  │
│ │  target_rate = 1000.0                                 │  │
│ │  ...                                                   │  │
│ │                                                        │  │
│ │ When unknown scope arrives, match pattern & create    │  │
│ └────────────────┬───────────────────────────────────────┘  │
│                  │                                           │
│ ┌────────────────▼───────────────────────────────────────┐  │
│ │ Discovery Layer (trait-based)                         │  │
│ │                                                        │  │
│ │ Implementations:                                      │  │
│ │  - StaticDiscovery (seed nodes from config/env)      │  │
│ │  - DockerSwarmDiscovery (Docker DNS/API)             │  │
│ │  - KubernetesDiscovery (K8s endpoints API)           │  │
│ │  - mDNSDiscovery (optional, opt-in)                  │  │
│ │                                                        │  │
│ │ Returns: Vec<SocketAddr> of potential seed nodes     │  │
│ └────────────────┬───────────────────────────────────────┘  │
│                  │                                           │
│                  ▼                                           │
│ ┌────────────────────────────────────────────────────────┐  │
│ │ Gossip Protocol (Chitchat library)                    │  │
│ │                                                        │  │
│ │ Algorithm: Scuttlebutt + Phi Accrual Failure Detect  │  │
│ │ Transport: UDP                                         │  │
│ │                                                        │  │
│ │ Gossip State (per node):                              │  │
│ │  {                                                     │  │
│ │    node_id: "sentinel-1",                             │  │
│ │    scopes: {                                          │  │
│ │      "api-endpoint": {                                │  │
│ │        request_rate: 45.2,                            │  │
│ │        accepted_rate: 40.1                            │  │
│ │      },                                               │  │
│ │      "login": { ... }                                 │  │
│ │    },                                                  │  │
│ │    timestamp: ...                                      │  │
│ │  }                                                     │  │
│ │                                                        │  │
│ │ Anti-entropy: Periodically sync full state            │  │
│ │ Phi failure detector: Detect dead vs slow nodes       │  │
│ └────────────────┬───────────────────────────────────────┘  │
│                  │                                           │
│                  │ Cluster secret authentication             │
│                  │ (TLS + pre-shared key)                    │
│                  ▼                                           │
└──────────────────────────────────────────────────────────────┘
                   │
                   │ UDP gossip to peers
                   ▼
            [Other Sentinel Nodes]
```

## Component Details

### 1. HTTP API Server

**Framework**: axum (lightweight, built on tokio)

**Endpoints**:

```
POST /should_throttle
Request:  {"scope": "api-endpoint"}
Response: {
  "should_throttle": false,
  "current_rate": 45.2,
  "target_rate": 100.0,
  "accepted_rate": 40.1
}

GET /health
Response: {
  "healthy": true,
  "peers": 5,
  "scopes": 12
}

GET /metrics
Response: (Prometheus text format)
# HELP nenya_requests_total Total requests checked
# TYPE nenya_requests_total counter
nenya_requests_total{scope="api-endpoint",throttled="false"} 1523
...
```

**Binding**: `127.0.0.1:8080` (localhost only, not exposed to network)

### 2. Rate Limit Manager

**Core responsibility**: Manage multiple independent rate limiters (scopes)

**Key operations**:
- `should_throttle(scope: &str) -> ThrottleDecision`
- Auto-create scopes on first use (using pattern matching)
- Update `external_request_rate` for each scope from gossip state
- Periodic cleanup of unused scopes (optional)

**Data structure**:
```rust
struct RateLimitManager {
    limiters: HashMap<String, RateLimiter<f64>>,
    patterns: Vec<ScopePattern>,
    cluster_state: Arc<RwLock<ClusterState>>,
}

struct ScopePattern {
    pattern: String,        // e.g., "api#*"
    config: RateLimitConfig,
}

struct RateLimitConfig {
    target_rate: f64,
    min_rate: f64,
    max_rate: f64,
    pid: PIDConfig,
}
```

### 3. Scope Pattern Matching

**Wildcard syntax**: Simple glob-style patterns
- `api-endpoint` - Exact match
- `api#*` - Matches any scope starting with "api#"
- `*` - Catch-all default

**Matching priority**:
1. Exact match
2. Most specific pattern (longest prefix)
3. Default pattern

**Example**:
```toml
[[rate_limits]]
pattern = "login"
target_rate = 50.0

[[rate_limits]]
pattern = "api#*"
target_rate = 100.0

[[rate_limits]]
pattern = "api#premium_*"
target_rate = 1000.0

[[rate_limits]]
pattern = "*"  # Default for unknown scopes
target_rate = 10.0
```

Request for `api#premium_customer1`:
1. No exact match
2. Matches both `api#*` and `api#premium_*`
3. Choose `api#premium_*` (more specific)
4. Create limiter with `target_rate = 1000.0`

### 4. Discovery Layer

**Trait definition**:
```rust
#[async_trait]
trait PeerDiscovery: Send + Sync {
    async fn discover_seeds(&self) -> Result<Vec<SocketAddr>>;
}
```

**Implementations**:

**StaticDiscovery**:
```rust
struct StaticDiscovery {
    seeds: Vec<SocketAddr>,
}
// From env: NENYA_SEED_NODES=10.0.1.5:8081,10.0.1.6:8081
// From TOML: seed_nodes = ["10.0.1.5:8081", "10.0.1.6:8081"]
```

**DockerSwarmDiscovery**:
```rust
struct DockerSwarmDiscovery {
    service_name: String,
}
// Query Docker DNS for all IPs in service
// Or use Docker API: /services/{name}/tasks
```

**KubernetesDiscovery**:
```rust
struct KubernetesDiscovery {
    namespace: String,
    label_selector: String,
}
// Query K8s API: /api/v1/namespaces/{ns}/endpoints
// Or use DNS: nenya-sentinel.default.svc.cluster.local
```

**Discovery aggregation**:
```rust
// Combine multiple discovery methods
let seeds = static_discovery.discover_seeds().await?
    .chain(docker_discovery.discover_seeds().await?)
    .collect();
```

### 5. Gossip Protocol

**Library**: [Chitchat](https://quickwit.io/blog/chitchat)

**Why Chitchat over SWIM**:
- Anti-entropy approach (better reliability for state propagation)
- No missed messages (critical for rate limit state)
- Battle-tested algorithm (Apache Cassandra uses similar approach)
- Phi accrual failure detection (distinguishes slow from dead nodes)

**Gossip state schema**:
```rust
struct NodeState {
    node_id: String,
    scopes: HashMap<String, ScopeState>,
}

struct ScopeState {
    request_rate: f64,
    accepted_rate: f64,
    timestamp: SystemTime,
}
```

**State aggregation**:
```rust
// For each scope, sum rates from all alive peers
fn aggregate_peer_rates(&self, scope: &str) -> f64 {
    self.cluster_state
        .alive_peers()
        .filter_map(|peer| peer.scopes.get(scope))
        .map(|state| state.accepted_rate)
        .sum()
}
```

**Gossip interval**: ~1 second (configurable)

**Failure detection**: Phi threshold (default: 8.0)

### 6. Security Model

**Cluster authentication**:
```rust
// Cluster secret loaded from:
// 1. File: /run/secrets/nenya_cluster_secret (Docker/K8s secrets)
// 2. Env:  NENYA_CLUSTER_SECRET
// 3. TOML: cluster_secret_file = "/path/to/secret"

struct ClusterAuth {
    secret: String,
}

// During gossip handshake:
// 1. TLS connection (prevents MITM)
// 2. Challenge-response with cluster secret (prevents unauthorized nodes)
```

**Discovery vs. Authentication**:
- Discovery is unauthenticated (finds candidates)
- Gossip join requires authentication (filters trusted nodes)
- Malicious nodes can't join without cluster secret

## Configuration

### Environment Variables

```bash
# Required
NENYA_CLUSTER_SECRET=your-secret-token

# Optional (with defaults)
NENYA_LISTEN_ADDR=127.0.0.1:8080       # Client API address
NENYA_GOSSIP_ADDR=0.0.0.0:8081         # Gossip protocol address
NENYA_DISCOVERY=static                  # Discovery method
NENYA_SEED_NODES=host1:8081,host2:8081 # Static seed nodes

# Simple rate limit defaults (if no TOML file)
NENYA_DEFAULT_TARGET_RATE=100.0
NENYA_DEFAULT_MIN_RATE=10.0
NENYA_DEFAULT_MAX_RATE=1000.0
```

### TOML Configuration

**File locations** (checked in order):
1. `./nenya.toml`
2. `/etc/nenya/nenya.toml`
3. Path from `NENYA_CONFIG` env var

**Example configuration**:
```toml
# Cluster configuration
cluster_secret_file = "/run/secrets/nenya_cluster_secret"
node_id = "sentinel-1"  # Optional, auto-generated if not provided

# Network configuration
listen_addr = "127.0.0.1:8080"
gossip_addr = "0.0.0.0:8081"

# Discovery configuration
[discovery]
method = "docker-swarm"  # or "kubernetes", "static", "mdns"
service_name = "nenya-sentinel"  # For Docker Swarm
# namespace = "default"  # For Kubernetes
# label_selector = "app=nenya"  # For Kubernetes

# Static seed nodes (fallback)
seed_nodes = ["10.0.1.5:8081", "10.0.1.6:8081"]

# Gossip configuration
[gossip]
interval_ms = 1000
phi_threshold = 8.0

# Rate limit patterns
[[rate_limits]]
pattern = "login"
target_rate = 50.0
min_rate = 25.0
max_rate = 100.0

[rate_limits.pid]
kp = 0.8
ki = 0.05
kd = 0.04
error_bias = 0.0
error_limit = 10.0
output_limit = 3.0

[[rate_limits]]
pattern = "api#*"
target_rate = 100.0
min_rate = 50.0
max_rate = 200.0

[[rate_limits]]
pattern = "api#premium_*"
target_rate = 1000.0
min_rate = 500.0
max_rate = 2000.0

# Default for unknown scopes
[[rate_limits]]
pattern = "*"
target_rate = 10.0
min_rate = 5.0
max_rate = 20.0
```

## Observability

### Metrics (Prometheus)

**Exposed at**: `GET /metrics`

**Key metrics**:
```
# Requests
nenya_requests_total{scope, throttled} counter
nenya_request_rate{scope} gauge
nenya_accepted_rate{scope} gauge

# Rate limiter state
nenya_target_rate{scope} gauge
nenya_pid_correction{scope} gauge

# Cluster state
nenya_cluster_peers gauge
nenya_cluster_scopes gauge
nenya_gossip_messages_total{type} counter

# Health
nenya_up gauge
```

### Tracing (OpenTelemetry)

**Library**: `tracing` crate with `#[instrument]` macros

**Key spans**:
- `should_throttle` - Full request lifecycle
- `gossip_sync` - Gossip state synchronization
- `peer_discovery` - Discovery operations
- `scope_creation` - Auto-creation of new scopes

**Export**: OTLP to collector (Jaeger, Tempo, etc.)

**Configuration**:
```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

## Deployment Patterns

### Docker Sidecar

```yaml
version: '3.8'
services:
  app:
    image: myapp:latest
    depends_on:
      - nenya

  nenya:
    image: nenya-sentinel:latest
    environment:
      NENYA_CLUSTER_SECRET: ${CLUSTER_SECRET}
      NENYA_DISCOVERY: docker-swarm
    secrets:
      - nenya_cluster_secret
    networks:
      - app-network

secrets:
  nenya_cluster_secret:
    external: true
```

### Kubernetes

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: myapp
spec:
  template:
    spec:
      containers:
      - name: app
        image: myapp:latest

      - name: nenya
        image: nenya-sentinel:latest
        env:
        - name: NENYA_CLUSTER_SECRET
          valueFrom:
            secretKeyRef:
              name: nenya-cluster-secret
              key: cluster_secret
        - name: NENYA_DISCOVERY
          value: "kubernetes"
        ports:
        - containerPort: 8080
          name: http
        - containerPort: 8081
          name: gossip
```

### Traditional VMs / Bare Metal

```bash
# Install nenya-sentinel binary
curl -L https://github.com/emersonmde/nenya/releases/download/v1.0.0/nenya-sentinel -o /usr/local/bin/nenya-sentinel
chmod +x /usr/local/bin/nenya-sentinel

# Create config
cat > /etc/nenya/nenya.toml <<EOF
cluster_secret_file = "/etc/nenya/cluster.secret"
seed_nodes = ["10.0.1.5:8081", "10.0.1.6:8081"]

[[rate_limits]]
pattern = "*"
target_rate = 100.0
EOF

# Create systemd service
cat > /etc/systemd/system/nenya-sentinel.service <<EOF
[Unit]
Description=Nenya Distributed Rate Limiter
After=network.target

[Service]
ExecStart=/usr/local/bin/nenya-sentinel --config /etc/nenya/nenya.toml
Restart=always

[Install]
WantedBy=multi-user.target
EOF

systemctl enable nenya-sentinel
systemctl start nenya-sentinel
```

## Testing Strategy

### Unit Tests
- Existing tests for `nenya` library (PID controller, rate limiter logic)
- Pattern matching logic
- Scope auto-creation
- Configuration parsing

### Integration Tests
- Multi-node gossip (spawn multiple sentinel instances locally)
- Discovery mechanisms (mock Docker/K8s APIs)
- Rate limit coordination (verify distributed throttling works)
- Network partition scenarios (pause gossip, verify degradation)
- Scope synchronization across nodes

### Test Helpers
```rust
// tests/integration/helpers.rs
struct TestCluster {
    nodes: Vec<SentinelNode>,
}

impl TestCluster {
    async fn spawn(count: usize) -> Self {
        // Spawn N sentinel instances on random ports
    }

    async fn partition(&mut self, group1: &[usize], group2: &[usize]) {
        // Create network partition between groups
    }

    async fn heal_partition(&mut self) {
        // Remove network partition
    }
}
```

### CI/CD Integration
- All tests runnable via `cargo test`
- No external dependencies (mock Docker/K8s APIs)
- Deterministic (no flaky tests due to timing)
- Fast (<30 seconds for full suite)

## Failure Modes

### Network Partition
**Scenario**: Gossip fails between node groups

**Behavior**:
- Each partition continues with last known peer state
- PID controllers adapt based on local + stale remote rates
- May allow temporary overage in isolated partition
- Automatically recovers when partition heals

**Mitigation**: Configurable staleness timeout (discard peer state older than N seconds)

### Node Failure
**Scenario**: Sentinel node crashes or becomes unreachable

**Behavior**:
- Phi accrual detector marks node as dead
- Other nodes stop including its rates in aggregation
- Cluster capacity decreases proportionally
- PID controllers adapt to new total rate

**Recovery**: When node restarts, rejoins via seed nodes, begins fresh

### Rapid Scaling
**Scenario**: 10 new nodes join simultaneously (autoscaling event)

**Behavior**:
- New nodes connect to any available seed
- Receive full membership list via gossip
- Begin participating immediately
- Gossip protocol converges in O(log N) time

**Performance**: Chitchat handles thousands of nodes efficiently

### Cluster Secret Compromise
**Scenario**: Cluster secret leaked

**Mitigation**:
- Rotate secret (update in secrets manager)
- Rolling restart of all nodes with new secret
- Nodes with old secret fail authentication, can't join

## Performance Characteristics

### Latency
- `should_throttle` call: <1ms (local in-memory decision)
- Gossip propagation: ~1-2 seconds (configurable interval)
- State convergence: O(log N) gossip rounds

### Throughput
- Single node: 100k+ req/sec (limited by HTTP parsing, not rate limiting logic)
- Horizontal scaling: Linear (each node handles its local traffic)

### Memory
- Per scope: ~1KB (sliding window + PID state)
- 10k scopes: ~10MB per node
- Gossip state: O(N × M) where N = nodes, M = average scopes per node

### Network
- Gossip bandwidth: O(log N) messages per interval
- Message size: ~1KB per node state
- 100 nodes, 1s interval: ~100KB/s per node

## Open Questions / Future Work

1. **Scope cleanup**: Should unused scopes be garbage collected? When?
2. **State persistence**: Should sentinel persist state to disk for faster recovery?
3. **Dynamic reconfiguration**: Should rate limit configs be modifiable at runtime (beyond auto-creation)?
4. **Multi-cluster support**: Should we support multiple independent clusters on same network?
5. **Rate limit sharing vs. independent**: Current design is independent scopes. Should we support shared capacity pools?
6. **Admin API**: Do we need APIs to inspect/modify cluster state?
