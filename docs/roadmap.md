# Nenya Development Roadmap

This document outlines the implementation plan for Nenya distributed rate limiting, broken into milestones with specific tasks.

## Current Milestone

**Status**: Ready for Milestone 1 - Single-Node HTTP Rate Limiter

**Library Status**: ✅ Distributed coordination support complete (equal division PID)

See Milestone 1 tasks below for what needs to be completed.

## Principles

- **Iterative development**: Each milestone produces a working, testable system
- **Test-driven**: Write tests alongside implementation
- **No regressions**: All tests must pass before completing milestone
- **Commit at milestone completion**: Push working code at the end of each milestone

## Workflow

1. Start Claude Code
2. Identify current milestone from this roadmap
3. Develop implementation plan (can use plan mode)
4. Implement tasks with tests
5. Verify all tests pass: `cargo test && cargo fmt --check && cargo clippy`
6. Push commit(s) at milestone completion
7. Check off milestone in this file
8. Move to next milestone

---

## Milestone 0: Preparation & Cleanup

- [ ] **MILESTONE COMPLETE**

**Goal**: Prepare the codebase for distributed features by removing gRPC and adding HTTP stack.

**Architecture Reference**: See [docs/architecture.md](architecture.md) - HTTP API Server section

### Tasks

- [ ] **Remove gRPC/protobuf from nenya-sentinel**
  - Remove `tonic`, `tonic-build`, `prost` dependencies
  - Delete `proto/sentinel.proto`
  - Delete `build.rs`
  - Clean up any proto-generated code

- [ ] **Add HTTP framework**
  - Add `axum` and `tokio` to nenya-sentinel dependencies
  - Add `serde` and `serde_json` for JSON handling

- [ ] **Add observability crates**
  - Add `tracing`, `tracing-subscriber`, `tracing-opentelemetry`
  - Add `metrics`, `metrics-exporter-prometheus`

- [ ] **Project structure**
  - Create `nenya-sentinel/src/` subdirectories:
    - `api/` - HTTP API handlers
    - `manager/` - Rate limit manager
    - `discovery/` - Discovery implementations
    - `config/` - Configuration loading
    - `observability/` - Metrics & tracing setup

**Deliverable**: Clean slate for nenya-sentinel with HTTP stack and project structure ready

**Verification**:
```bash
# Compilation
cargo build -p nenya-sentinel  # Should compile
cargo test -p nenya-sentinel   # Should pass (no tests yet, but shouldn't error)

# Library benchmarks (establish performance baseline)
cargo bench --bench rate_limiter_bench
cargo bench --bench pid_controller_bench

# Save baseline before binary work
cargo bench -- --save-baseline milestone-0
```

**Performance Baseline** (library only, from benchmarks):
- Hot path decision: ~40ns (target: <1μs) ✅
- PID computation: ~1-2ns (target: <100ns) ✅
- Throughput: 25M decisions/sec single-threaded

**Commit Message**: `Milestone 0: Prepare HTTP stack for nenya-sentinel`

---

## Milestone 1: Single-Node Foundation

- [ ] **MILESTONE COMPLETE**

**Goal**: Build a working single-node HTTP rate limiter (no distribution yet).

**Architecture Reference**: See [docs/architecture.md](architecture.md):
- HTTP API Server section
- Configuration section
- Rate Limit Manager section

### Tasks

#### 1.1 Configuration System

- [ ] **Define configuration schema**
  - Create `config/mod.rs` with config structs
  - Support TOML file parsing (`toml` crate)
  - Support environment variable overrides (`envy` or manual)
  - Implement config file search (./nenya.toml, /etc/nenya/nenya.toml, NENYA_CONFIG)

- [ ] **Cluster secret loading**
  - Load from file (`/run/secrets/nenya_cluster_secret`)
  - Load from env (`NENYA_CLUSTER_SECRET`)
  - Load from TOML (`cluster_secret_file`)
  - Error if no secret provided

- [ ] **Pattern-based rate limit configs**
  - Support `[[rate_limits]]` array in TOML
  - Implement simple wildcard matching (`*` suffix)
  - Default pattern fallback

**Tests**: Config parsing, pattern matching, secret loading

#### 1.2 HTTP API Server

- [ ] **Basic axum server**
  - Bind to `127.0.0.1:8080` (localhost only)
  - Add graceful shutdown on SIGTERM/SIGINT
  - Add request logging with tracing

- [ ] **POST /should_throttle endpoint**
  ```rust
  struct ThrottleRequest { scope: String }
  struct ThrottleResponse {
      should_throttle: bool,
      current_rate: f64,
      target_rate: f64,
      accepted_rate: f64,
  }
  ```
  - Parse JSON request
  - Call rate limit manager
  - Return JSON response

- [ ] **GET /health endpoint**
  ```rust
  struct HealthResponse {
      healthy: bool,
      scopes: usize,
      peers: usize,  // Always 1 in Phase 1
  }
  ```

- [ ] **GET /metrics endpoint**
  - Prometheus text format exporter
  - Basic metrics (requests_total, up)

**Tests**: HTTP endpoint integration tests using reqwest

#### 1.3 Rate Limit Manager

- [ ] **RateLimitManager struct**
  - `HashMap<String, RateLimiter<f64>>` for scopes
  - Pattern matcher for auto-creation
  - Integration with existing `nenya` crate

- [ ] **should_throttle logic**
  - Check if scope exists
  - If not, match pattern and create
  - Call `limiter.should_throttle()`
  - Return decision + metadata

- [ ] **Metrics instrumentation**
  - `nenya_requests_total{scope, throttled}` counter
  - `nenya_request_rate{scope}` gauge
  - `nenya_target_rate{scope}` gauge

**Tests**:
- Scope auto-creation
- Pattern matching priority
- Rate limiting behavior (single node)
- Metrics collection

#### 1.4 Observability

- [ ] **Tracing setup**
  - Initialize `tracing_subscriber`
  - Configure log levels from env (RUST_LOG)
  - Add `#[instrument]` to key functions

- [ ] **Prometheus metrics**
  - Expose `/metrics` endpoint
  - Register custom metrics
  - Update metrics in rate limit manager

**Tests**: Metrics endpoint returns valid Prometheus format

**Deliverable**: Working single-node rate limiter with HTTP API, scope auto-creation, and observability

**Testing Requirements**:

1. **Unit Tests** (existing + new):
   ```bash
   cargo test --lib                    # Library tests (170 existing)
   cargo test -p nenya-sentinel        # Binary tests (new)
   ```

2. **Integration Tests** (new):
   ```bash
   cargo test --test '*'               # HTTP API tests
   ```
   - Test POST /should_throttle with various scopes
   - Test pattern matching (exact, wildcard, default)
   - Test config loading (TOML, env vars)
   - Test auto-creation of new scopes
   - Test health and metrics endpoints

3. **HTTP Micro-Benchmarks** (new):
   Create `nenya-sentinel/benches/http_api_bench.rs`:
   ```rust
   // Benchmark: End-to-end HTTP request latency (without network)
   // - JSON deserialize
   // - Manager lookup
   // - Rate limiter decision
   // - JSON serialize
   // - HTTP response
   //
   // Target: <1ms p99 (leaves ~960μs budget for HTTP/JSON overhead)
   ```

   Run benchmarks:
   ```bash
   cargo bench -p nenya-sentinel --bench http_api_bench

   # Compare against baseline
   cargo bench -p nenya-sentinel -- --baseline milestone-0
   ```

   **Expected Results**:
   - Handler latency: <500μs p99
   - Total budget: Library (40ns) + Handler (<500μs) + Network (~500μs) = <1ms

4. **Manual Smoke Test**:
   ```bash
   # Start sentinel
   cargo run -p nenya-sentinel

   # In another terminal, test the API
   curl -X POST http://localhost:8080/should_throttle \
     -d '{"scope":"test"}' \
     -H "Content-Type: application/json"
   # Should return: {"should_throttle":false,"current_rate":0.0,"target_rate":10.0,"accepted_rate":0.0}

   curl http://localhost:8080/health
   # Should return: {"healthy":true,"scopes":1,"peers":1}

   curl http://localhost:8080/metrics
   # Should return Prometheus metrics
   ```

**Performance Targets (Milestone 1)**:
- Library decision: ~40ns (established)
- HTTP handler latency: <500μs p99
- JSON serialization: <100μs
- End-to-end: <1ms p99 (excluding network)

**Verification Checklist**:
```bash
# 1. All tests pass
cargo test --all

# 2. Benchmarks meet targets
cargo bench --all

# 3. No clippy warnings
cargo clippy --all-targets --all-features -- -D warnings

# 4. Code formatted
cargo fmt --check

# 5. Manual API test (smoke test)
# Start server and run curl commands above
```

**Commit Message**: `Milestone 1: Single-node HTTP rate limiter with scope auto-creation`

---

## Milestone 2: Gossip Integration

- [ ] **MILESTONE COMPLETE**

**Goal**: Add distributed coordination using Chitchat gossip protocol.

**Architecture Reference**: See [docs/architecture.md](architecture.md):
- Gossip Protocol section
- Distributed Coordination explanation

### Tasks

#### 2.1 Chitchat Integration

- [ ] **Add Chitchat dependency**
  - Add to `nenya-sentinel/Cargo.toml`
  - Study Chitchat API and examples

- [ ] **Gossip manager module**
  - Create `gossip/mod.rs`
  - Initialize Chitchat cluster
  - Configure gossip address (bind to `0.0.0.0:8081`)
  - Handle cluster membership events

- [ ] **State schema design**
  - Define gossip state format:
    ```rust
    struct NodeState {
        node_id: String,
        scopes: HashMap<String, ScopeRates>,
    }
    struct ScopeRates {
        request_rate: f64,
        accepted_rate: f64,
        timestamp: SystemTime,
    }
    ```
  - Serialize/deserialize with serde

- [ ] **State publication**
  - Periodically publish local scope rates to Chitchat
  - Update interval: 1 second (configurable)

- [ ] **State consumption**
  - Subscribe to peer state updates
  - Aggregate peer rates per scope
  - Update `external_request_rate` in RateLimitManager

**Tests**:
- State serialization
- Gossip state aggregation logic (unit tests)

#### 2.2 Multi-Node Integration Tests

- [ ] **Test harness**
  - Create `tests/integration/cluster.rs`
  - Helper to spawn N sentinel processes
  - Helper to make HTTP requests to each node
  - Helper to wait for gossip convergence

- [ ] **Basic gossip test**
  - Spawn 3 nodes
  - Make requests to node 1 for scope "test"
  - Verify node 2 and 3 see updated rates via gossip
  - Verify distributed throttling works

- [ ] **Node join test**
  - Start 2 nodes
  - Add 3rd node
  - Verify new node receives full state
  - Verify new node participates in rate limiting

- [ ] **Node failure test**
  - Start 3 nodes
  - Kill node 1
  - Verify nodes 2 and 3 detect failure
  - Verify rate limiting continues with 2 nodes

**Tests**: Multi-node integration tests (spawn real processes)

**Deliverable**: Multi-node distributed rate limiting with gossip coordination

**Verification**:
```bash
# All tests pass including integration tests
cargo test

# Integration test specifically
cargo test --test cluster

# Manual verification: Start 3 nodes
cargo run -p nenya-sentinel -- --gossip-addr 127.0.0.1:8081 &
cargo run -p nenya-sentinel -- --listen-addr 127.0.0.1:8090 --gossip-addr 127.0.0.1:8091 --seed-nodes 127.0.0.1:8081 &
cargo run -p nenya-sentinel -- --listen-addr 127.0.0.1:8100 --gossip-addr 127.0.0.1:8101 --seed-nodes 127.0.0.1:8081 &

# Send requests to node 1
for i in {1..20}; do curl -X POST http://localhost:8080/should_throttle -d '{"scope":"test"}' -H "Content-Type: application/json"; done

# Check node 2 sees the rate
curl http://localhost:8090/health
# Should show peers: 3

# Kill all background jobs
jobs -p | xargs kill
```

**Testing Requirements**:

1. **Unit & Integration Tests**:
   ```bash
   cargo test --all  # Include new gossip tests
   ```
   - Test gossip state serialization/deserialization
   - Test external rate aggregation
   - Test cluster membership events
   - Test rate synchronization between nodes

2. **Multi-Node Integration Test** (new):
   Create `tests/distributed_coordination.rs`:
   ```rust
   // Test: 3-node cluster coordinating on shared scope
   // - Start 3 nodes programmatically
   // - Send 150 TPS across all nodes (50 each)
   // - Target: 100 TPS cluster-wide
   // - Verify: Total accepted ~100 TPS across cluster
   // - Verify: Gossip propagates rates within 1s
   ```

3. **Load Tests** (new - basic):
   Create `load-tests/constant_load.rs`:
   ```bash
   # Install wrk2 (constant RPS load generator)
   # macOS: brew install wrk2
   # Linux: https://github.com/giltene/wrk2

   # Test: Single node handling constant load
   wrk2 -t2 -c10 -d30s -R1000 \
     --latency \
     -s load-tests/throttle.lua \
     http://localhost:8080/should_throttle

   # Expected results:
   # - Latency p50: <1ms
   # - Latency p99: <5ms
   # - Latency p99.9: <10ms
   # - Success rate: 100%
   ```

   Create `load-tests/throttle.lua`:
   ```lua
   wrk.method = "POST"
   wrk.headers["Content-Type"] = "application/json"
   wrk.body = '{"scope":"load-test"}'
   ```

4. **Gossip Overhead Benchmark** (new):
   Create `nenya-sentinel/benches/gossip_bench.rs`:
   ```rust
   // Benchmark: Gossip state update impact on decision latency
   // - Measure without gossip (baseline)
   // - Measure with 2 peers gossiping
   // - Measure with 10 peers gossiping
   //
   // Target: Gossip overhead <100μs per decision
   ```

5. **Network Partition Test** (manual):
   ```bash
   # Start 3 nodes
   # Use iptables/pfctl to block gossip between nodes
   # Verify: Nodes continue accepting with stale data
   # Restore network
   # Verify: Nodes re-sync within 2-3 gossip rounds
   ```

**Performance Targets (Milestone 2)**:
- Single-node latency: <1ms p99 (no regression from M1)
- Gossip overhead: <100μs per decision
- Rate convergence: <2s for cluster-wide changes
- Network partition: Graceful degradation, auto-recovery

**Verification Checklist**:
```bash
# 1. All tests pass (including new distributed tests)
cargo test --all

# 2. Benchmarks verify no regression
cargo bench --all

# 3. Load test: 1K RPS sustained for 30s
wrk2 -t2 -c10 -d30s -R1000 --latency \
  -s load-tests/throttle.lua \
  http://localhost:8080/should_throttle
# Check: p99 <5ms, success rate 100%

# 4. Multi-node manual test (3 nodes, see above)

# 5. Code quality checks
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

**Commit Message**: `Milestone 2: Distributed rate limiting with Chitchat gossip`

---

## Milestone 3: Discovery Implementations

- [ ] **MILESTONE COMPLETE**

**Goal**: Add automatic peer discovery for Docker Swarm and Kubernetes.

**Architecture Reference**: See [docs/architecture.md](architecture.md):
- Discovery Layer section
- Deployment Patterns section

### Tasks

#### 3.1 Discovery Trait

- [ ] **Define trait**
  ```rust
  #[async_trait]
  trait PeerDiscovery: Send + Sync {
      async fn discover_seeds(&self) -> Result<Vec<SocketAddr>>;
  }
  ```

- [ ] **Static discovery**
  - Load from `seed_nodes` in config
  - Load from `NENYA_SEED_NODES` env var
  - Always available as fallback

**Tests**: Static discovery from config and env vars

#### 3.2 Docker Swarm Discovery

- [ ] **Docker API client**
  - Use `bollard` crate (Docker API client)
  - Query services API for task IPs
  - Filter by service name

- [ ] **DNS-based discovery**
  - Query DNS for service name (tasks.<service>.swarm)
  - Parse DNS responses to get IPs

- [ ] **Configuration**
  ```toml
  [discovery]
  method = "docker-swarm"
  service_name = "nenya-sentinel"
  ```

**Tests**:
- Mock Docker API responses
- Integration test with real Docker Swarm (optional, can be manual)

#### 3.3 Kubernetes Discovery

- [ ] **K8s API client**
  - Use `kube` crate
  - Query endpoints API for pod IPs
  - Filter by label selector

- [ ] **DNS-based discovery**
  - Query headless service DNS
  - Get all pod IPs

- [ ] **Configuration**
  ```toml
  [discovery]
  method = "kubernetes"
  namespace = "default"
  label_selector = "app=nenya"
  ```

**Tests**:
- Mock K8s API responses
- Integration test with real K8s cluster (optional, can be manual)

#### 3.4 mDNS Discovery (Optional)

- [ ] **mDNS implementation**
  - Use `mdns` crate
  - Advertise service: `_nenya._tcp.local`
  - Discover peers via mDNS query

- [ ] **Opt-in only**
  ```toml
  [discovery]
  method = "mdns"
  service_name = "_nenya._tcp.local"
  ```

**Tests**: mDNS discovery in isolated network

**Deliverable**: Automatic peer discovery for Docker Swarm and Kubernetes

**Verification**:
```bash
# All tests pass
cargo test

# Docker Swarm discovery test (requires Docker)
# See manual test instructions in architecture.md - Deployment Patterns section

# Kubernetes discovery test (requires K8s cluster)
# See manual test instructions in architecture.md - Deployment Patterns section

# Static discovery still works
cargo run -p nenya-sentinel -- --seed-nodes 127.0.0.1:8081
```

**Commit Message**: `Milestone 3: Add Docker Swarm and Kubernetes discovery`

---

## Milestone 4: Security & Authentication

- [ ] **MILESTONE COMPLETE**

**Goal**: Secure gossip protocol with cluster secret authentication.

**Architecture Reference**: See [docs/architecture.md](architecture.md):
- Security Model section
- Configuration section (cluster secret loading)

### Tasks

#### 4.1 Cluster Secret Authentication

- [ ] **Handshake protocol**
  - Challenge-response during gossip join
  - Include HMAC of challenge with cluster secret
  - Reject nodes with incorrect secret

- [ ] **Integration with Chitchat**
  - Hook into Chitchat's join/authentication
  - May need to wrap Chitchat transport layer

**Tests**:
- Node with correct secret joins successfully
- Node with incorrect secret is rejected
- Tampered handshake is rejected

#### 4.2 TLS for Gossip (Optional)

- [ ] **TLS transport layer**
  - Wrap UDP/TCP with TLS
  - Self-signed certificates or custom CA
  - Mutual TLS (mTLS) for node-to-node auth

- [ ] **Configuration**
  ```toml
  [gossip.tls]
  enabled = true
  cert_file = "/path/to/cert.pem"
  key_file = "/path/to/key.pem"
  ca_file = "/path/to/ca.pem"
  ```

**Tests**: TLS handshake, certificate validation

**Deliverable**: Secure cluster membership with authentication

**Verification**:
```bash
# All tests pass including security tests
cargo test

# Test with cluster secret
export NENYA_CLUSTER_SECRET="test-secret-123"
cargo run -p nenya-sentinel &
cargo run -p nenya-sentinel -- --listen-addr 127.0.0.1:8090 --gossip-addr 127.0.0.1:8091 --seed-nodes 127.0.0.1:8081 &

# Nodes should join successfully
curl http://localhost:8080/health  # Should show peers: 2

# Test with wrong secret (should fail to join)
NENYA_CLUSTER_SECRET="wrong-secret" cargo run -p nenya-sentinel -- --listen-addr 127.0.0.1:8100 --gossip-addr 127.0.0.1:8101 --seed-nodes 127.0.0.1:8081 &

# Original nodes should still show peers: 2 (unauthorized node rejected)
curl http://localhost:8080/health

# Kill all
jobs -p | xargs kill
```

**Commit Message**: `Milestone 4: Add cluster secret authentication`

---

## Milestone 5: Production Hardening

- [ ] **MILESTONE COMPLETE**

**Goal**: Make nenya-sentinel production-ready with hardening, performance optimization, and documentation.

**Architecture Reference**: See [docs/architecture.md](architecture.md):
- Failure Modes section
- Performance Characteristics section
- Deployment Patterns section

### Tasks

#### 5.1 Error Handling

- [ ] **Graceful degradation**
  - Handle gossip failures without crashing
  - Continue local rate limiting if gossip unavailable
  - Exponential backoff for discovery retries

- [ ] **Logging and error messages**
  - Clear error messages for common misconfigurations
  - Helpful startup logs (bound addresses, discovered peers, etc.)

**Tests**:
- Network failures don't crash process
- Discovery failures fall back to static seeds

#### 5.2 Performance Optimization

- [ ] **Benchmark HTTP throughput**
  - Use `criterion` for benchmarks
  - Target: >50k req/sec per node

- [ ] **Optimize gossip overhead**
  - Tune gossip interval
  - Compress gossip messages if needed

- [ ] **Memory profiling**
  - Test with 10k+ scopes
  - Ensure no memory leaks

**Tests**: Performance benchmarks in CI

#### 5.3 Operational Features

- [ ] **Scope cleanup** (optional)
  - Remove scopes with no requests for N minutes
  - Configurable TTL

- [ ] **Graceful shutdown**
  - Drain in-flight requests
  - Notify peers before leaving cluster
  - Clean exit on SIGTERM

- [ ] **Health checks**
  - `/health` returns unhealthy if no peers (and discovery expected peers)
  - `/health` returns unhealthy if gossip stale (no updates for >10s)

**Tests**:
- Graceful shutdown test
- Health check accuracy

#### 5.4 Documentation

- [ ] **API documentation**
  - OpenAPI/Swagger spec for HTTP API
  - Document all config options
  - Example TOML configs for each platform

- [ ] **Deployment guides**
  - Docker Compose example
  - Kubernetes manifests
  - Systemd service file
  - AWS ECS task definition

- [ ] **Runbook**
  - Common issues and solutions
  - How to rotate cluster secret
  - How to debug gossip issues

**Deliverable**: Production-ready v1.0.0 release

**Testing Requirements (Comprehensive)**:

1. **All Existing Tests Pass**:
   ```bash
   cargo test --all --all-features
   cargo clippy --all-targets --all-features -- -D warnings
   cargo fmt --check
   cargo audit  # No security vulnerabilities
   ```

2. **Benchmarks Meet Targets**:
   ```bash
   # Library benchmarks (baseline: ~40ns decision, ~1-2ns PID)
   cargo bench --bench rate_limiter_bench
   cargo bench --bench pid_controller_bench

   # HTTP API benchmarks (target: <500μs handler latency)
   cargo bench -p nenya-sentinel --bench http_api_bench

   # Gossip benchmarks (target: <100μs overhead)
   cargo bench -p nenya-sentinel --bench gossip_bench

   # Compare against Milestone 0 baseline
   cargo bench -- --baseline milestone-0
   # Verify: No significant regressions
   ```

3. **Load Testing Suite** (comprehensive):

   **a) Single-Node Constant Load**:
   ```bash
   # Start server
   cargo run --release -p nenya-sentinel &

   # Test: 1K RPS sustained for 5 minutes
   wrk2 -t4 -c20 -d300s -R1000 \
     --latency \
     -s load-tests/throttle.lua \
     http://localhost:8080/should_throttle

   # Expected results:
   # - p50: <1ms
   # - p95: <2ms
   # - p99: <5ms
   # - p99.9: <10ms
   # - p99.99: <50ms
   # - Success rate: 100%
   # - Sustained throughput: 1000 RPS

   kill %1  # Stop server
   ```

   **b) Single-Node High Throughput**:
   ```bash
   # Test: Maximum throughput (target: >50K RPS)
   wrk2 -t8 -c100 -d60s -R100000 \
     --latency \
     -s load-tests/throttle.lua \
     http://localhost:8080/should_throttle

   # Expected: Sustains >50K RPS with p99 <10ms
   ```

   **c) Multi-Scope Load**:
   ```bash
   # Test: 1000 different scopes, 10 RPS each = 10K total RPS
   # Create load-tests/multi_scope.lua
   wrk2 -t4 -c40 -d120s -R10000 \
     --latency \
     -s load-tests/multi_scope.lua \
     http://localhost:8080/should_throttle

   # Verify: All scopes properly tracked, no memory leaks
   ```

   **d) Distributed Load (3 nodes)**:
   ```bash
   # Start 3-node cluster (in separate terminals)
   cargo run --release -p nenya-sentinel -- \
     --listen-addr 127.0.0.1:8080 --gossip-addr 127.0.0.1:8081 &

   cargo run --release -p nenya-sentinel -- \
     --listen-addr 127.0.0.1:8090 --gossip-addr 127.0.0.1:8091 \
     --seed-nodes 127.0.0.1:8081 &

   cargo run --release -p nenya-sentinel -- \
     --listen-addr 127.0.0.1:8100 --gossip-addr 127.0.0.1:8101 \
     --seed-nodes 127.0.0.1:8081 &

   # Test: Load distributed across all nodes
   # Send 333 RPS to each node = 1000 RPS total
   wrk2 -t2 -c10 -d60s -R333 \
     --latency -s load-tests/throttle.lua http://localhost:8080/should_throttle &
   wrk2 -t2 -c10 -d60s -R333 \
     --latency -s load-tests/throttle.lua http://localhost:8090/should_throttle &
   wrk2 -t2 -c10 -d60s -R334 \
     --latency -s load-tests/throttle.lua http://localhost:8100/should_throttle &

   wait  # Wait for all load tests to complete

   # Verify:
   # - Total cluster accepts ~target rate (accounting for coordination)
   # - Gossip converges within 2-3s
   # - No node crashes or hangs

   jobs -p | xargs kill  # Clean up
   ```

   **e) Burst Load**:
   ```bash
   # Test: Normal load with periodic bursts
   # Create load-tests/burst_load.lua (alternates 500 RPS / 5000 RPS)
   cargo run --release -p nenya-sentinel &

   wrk2 -t4 -c50 -d180s \
     --latency \
     -s load-tests/burst_load.lua \
     http://localhost:8080/should_throttle

   # Verify: PID adapts to bursts, no crashes
   kill %1
   ```

   **f) Ramp-Up Load**:
   ```bash
   # Test: Gradual ramp from 100 to 10K RPS over 5 minutes
   # Create load-tests/ramp_load.sh
   ./load-tests/ramp_load.sh

   # Verify: Smooth scaling, no memory issues at high load
   ```

4. **Stress Testing**:
   ```bash
   # Test: 10,000 unique scopes under load
   wrk2 -t8 -c100 -d300s -R50000 \
     --latency \
     -s load-tests/stress_scopes.lua \
     http://localhost:8080/should_throttle

   # Monitor with:
   # - Memory usage (should be stable, no leaks)
   # - CPU usage (should be reasonable, <80%)
   # - Latency (should not degrade over time)
   ```

5. **Soak Testing** (optional but recommended):
   ```bash
   # Test: 24-hour sustained load at moderate rate
   nohup wrk2 -t4 -c20 -d86400s -R5000 \
     --latency \
     -s load-tests/throttle.lua \
     http://localhost:8080/should_throttle \
     > soak-test.log 2>&1 &

   # Check after 24 hours:
   # - No crashes
   # - No memory leaks
   # - Latency stable
   # - All metrics healthy
   ```

6. **Failure Scenario Testing**:
   ```bash
   # a) Network partition recovery
   # Start 3 nodes, block gossip, verify graceful degradation,
   # restore network, verify auto-recovery

   # b) Node crash and rejoin
   # Kill a node, verify cluster continues, restart node,
   # verify rejoin and sync

   # c) Discovery failure
   # Disable discovery, verify fallback to static seeds

   # d) Invalid config handling
   # Test with missing cluster secret, invalid TOML, etc.
   # Verify clear error messages
   ```

7. **Documentation Verification**:
   ```bash
   # Build release binary
   cargo build --release -p nenya-sentinel

   # Verify documentation complete
   ls docs/
   # Should have: architecture.md, roadmap.md, deployment/

   # Test deployment examples
   # - Docker Compose: docker-compose up -d
   # - Kubernetes: kubectl apply -f k8s/
   # - Systemd: systemctl start nenya

   # Verify runbook accuracy
   # - Walk through common issues
   # - Test secret rotation procedure
   ```

8. **CI/CD Pipeline**:
   ```bash
   # Ensure all checks pass in GitHub Actions:
   # - Tests (all platforms)
   # - Benchmarks
   # - Clippy
   # - Security audit
   # - Documentation build
   # - Docker image build

   git push origin main
   # GitHub Actions should be green
   ```

**Performance Targets (Milestone 5 - Production)**:
- Library decision: ~40ns (established)
- HTTP handler: <500μs p99
- End-to-end: <5ms p99 (including network)
- Throughput: >50K RPS per node
- Gossip overhead: <100μs per decision
- Memory: Stable under 100MB for 1K scopes
- CPU: <50% at 10K RPS

**Acceptance Criteria**:
- ✅ All tests pass (170 library + binary integration + distributed tests)
- ✅ All benchmarks meet targets
- ✅ Load tests: 1K RPS for 5min with p99 <5ms
- ✅ Load tests: >50K RPS sustained for 1min
- ✅ Multi-node: 3-node cluster coordinates properly
- ✅ Soak test: 24hr at 5K RPS (no crashes, no leaks)
- ✅ Documentation complete and accurate
- ✅ CI/CD green on all checks
- ✅ Zero clippy warnings, zero audit vulnerabilities

**Commit Message**: `Milestone 5: Production hardening and v1.0.0 preparation`

**Next Steps**: Tag and release v1.0.0, publish Docker images, announce release

---

## Milestone 6: Advanced Features (Future)

- [ ] **MILESTONE COMPLETE**

**Goal**: Additional capabilities based on user feedback and real-world usage.

**Architecture Reference**: See [docs/architecture.md](architecture.md) - Open Questions / Future Work

### Potential Features

- [ ] **State persistence**
  - Persist scope state to disk
  - Faster recovery after restart
  - SQLite or RocksDB backend

- [ ] **Dynamic reconfiguration**
  - Modify rate limits at runtime via API
  - Persist changes to config file

- [ ] **Shared capacity pools**
  - Multiple scopes share a total rate limit
  - Hierarchical limits (global → service → endpoint)

- [ ] **Priority/weights**
  - Some scopes get priority over others
  - Weighted fair queueing

- [ ] **Advanced metrics**
  - Histograms of throttle decisions
  - P50/P90/P99 latencies
  - Per-scope PID controller state

- [ ] **Admin API**
  - Inspect cluster state
  - Force scope creation
  - Manually mark nodes as dead

- [ ] **Multi-cluster support**
  - Run multiple independent clusters on same network
  - Cluster namespacing

- [ ] **Client libraries**
  - Python, Go, Node.js, Java HTTP clients
  - Simplified integration

**Note**: These are future enhancements, not required for v1.0.0. Prioritize based on user requests and production needs.

---

## Testing Strategy Summary

### Unit Tests
- Config parsing
- Pattern matching
- Rate limiter logic (existing)
- Gossip state aggregation

### Integration Tests
- HTTP API endpoints
- Multi-node gossip
- Discovery mechanisms
- Network partitions
- Node failures
- Authentication

### Performance Tests
- HTTP throughput benchmarks
- Gossip overhead measurement
- Memory usage with many scopes

### CI/CD Pipeline
```yaml
# .github/workflows/rust.yml
- Run unit tests: cargo test --lib
- Run integration tests: cargo test --test '*'
- Run benchmarks: cargo bench
- Format check: cargo fmt --check
- Lint: cargo clippy
- Security audit: cargo audit
- Build release binary
```

### Pre-commit Hooks
```bash
#!/bin/bash
# .git-hooks-pre-commit (already exists)
cargo test --all
cargo fmt --check
cargo clippy -- -D warnings
```

---

## Milestone Summary

| Milestone | Key Deliverable | Status |
|-----------|----------------|--------|
| 0 | Clean HTTP stack | ✅ Complete (library) |
| 1 | Working HTTP rate limiter | ⏳ Ready to Start |
| 2 | Distributed coordination | 🟡 Library Complete, Binary TODO |
| 3 | Platform integrations | 🔜 Not Started |
| 4 | Cluster authentication | 🔜 Not Started |
| 5 | Production-ready release | 🔜 Not Started |
| 6 | Advanced features | 🔜 Future |

**Legend**: ✅ Complete | ⏳ In Progress | 🔜 Not Started

---

## Success Criteria

### Milestone 1 Complete
- Single-node rate limiter works via HTTP API
- Scope auto-creation functional
- Metrics exposed
- All tests passing

### Milestone 2 Complete
- 3+ nodes coordinate via gossip
- Distributed throttling accurate within 5% of target
- Network partitions handled gracefully
- All tests passing

### Milestone 3 Complete
- Docker Swarm discovery working
- Kubernetes discovery working
- Auto-discovery functional end-to-end
- All tests passing

### Milestone 4 Complete
- Cluster secret authentication working
- Unauthorized nodes rejected
- All tests passing

### Milestone 5 Complete
- Performance targets met (>50k req/sec)
- Production documentation complete
- Deployment examples working
- All tests passing

### v1.0.0 Release Ready
- All phases complete
- CI/CD pipeline green
- Documentation complete
- Docker image published
- Binary releases available
- No known critical bugs

## Risk Mitigation

### Risks

1. **Chitchat learning curve**: Library might be harder to integrate than expected
   - **Mitigation**: Allocate extra time in Phase 2, study examples thoroughly

2. **Platform-specific discovery complexity**: Docker/K8s APIs might be unreliable
   - **Mitigation**: Always support static seeds as fallback

3. **Gossip performance**: State propagation might be too slow
   - **Mitigation**: Tune gossip interval, benchmark early in Phase 2

4. **Authentication complexity**: Securing gossip might require significant effort
   - **Mitigation**: Start with simple pre-shared key, add TLS as optional

5. **Integration test flakiness**: Timing-dependent tests might be unreliable
   - **Mitigation**: Use proper synchronization, generous timeouts, retries

## Next Steps

1. Review this roadmap and adjust as needed
2. Create GitHub issues for each major task
3. Begin Phase 0: Preparation & Cleanup
4. Set up project board to track progress
