#!/bin/bash
# Nenya Cluster Demo - One-command cluster visualization
#
# Usage:
#   ./demo.sh        # Start 3-node cluster + visualizer + load generator
#   ./demo.sh --help # Show help

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
CLUSTER_SECRET="demo-secret"
SCOPE="test"
DEBUG_LOGS=""

# Parse arguments
for arg in "$@"; do
    case $arg in
        --help)
            echo "Nenya Cluster Demo"
            echo ""
            echo "Usage:"
            echo "  ./demo.sh          Start 3-node cluster + visualizer + load"
            echo "  ./demo.sh --debug  Enable debug logging for gossip sync"
            echo "  ./demo.sh --help   Show this help"
            echo ""
            echo "What happens:"
            echo "  1. Builds the server (takes a moment)"
            echo "  2. Starts 3 cluster nodes"
            echo "  3. Waits for cluster convergence"
            echo "  4. Generates 150 TPS load (target: 100 TPS)"
            echo "  5. Opens visualization dashboard (GUI window)"
            echo ""
            echo "The dashboard shows:"
            echo "  • Real-time node health (green = optimal)"
            echo "  • Cluster throughput graph"
            echo "  • Network topology with live rates"
            echo ""
            echo "Press Ctrl+C to stop everything cleanly."
            exit 0
            ;;
        --debug)
            DEBUG_LOGS="RUST_LOG=nenya=debug"
            ;;
    esac
done

# PIDs to track
NODE_PIDS=()

# Cleanup function
cleanup() {
    echo ""
    echo -e "${YELLOW}Shutting down cluster...${NC}"

    for pid in "${NODE_PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    done

    # Wait a bit for graceful shutdown
    sleep 1

    # Force kill if needed
    for pid in "${NODE_PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            kill -9 "$pid" 2>/dev/null || true
        fi
    done

    echo -e "${GREEN}Cleanup complete${NC}"
    exit 0
}

# Set up trap for cleanup
trap cleanup SIGINT SIGTERM EXIT

echo -e "${BLUE}╔══════════════════════════════════════╗${NC}"
echo -e "${BLUE}║  Nenya Distributed Rate Limiter Demo ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════╝${NC}"
echo ""

# Build the server first
echo -e "${YELLOW}Building server...${NC}"
cargo build --features server --quiet 2>&1 | grep -v "warning:" || true
echo -e "${GREEN}✓ Build complete${NC}"
echo ""

# Start Node 0 (seed)
echo -e "${YELLOW}Starting Node 0 (seed)...${NC}"
env $DEBUG_LOGS \
NENYA_CLUSTER_SECRET="$CLUSTER_SECRET" \
NENYA_ENABLE_GOSSIP=1 \
NENYA_GOSSIP_ADDR=127.0.0.1:8081 \
    cargo run --features server --quiet 2>&1 | sed 's/^/[Node-0] /' &
NODE_PIDS+=($!)
sleep 2

# Start Node 1
echo -e "${YELLOW}Starting Node 1...${NC}"
env $DEBUG_LOGS \
NENYA_CLUSTER_SECRET="$CLUSTER_SECRET" \
NENYA_ENABLE_GOSSIP=1 \
NENYA_LISTEN_ADDR=127.0.0.1:8090 \
NENYA_GOSSIP_ADDR=127.0.0.1:8091 \
NENYA_SEED_NODES=127.0.0.1:8081 \
NENYA_NODE_ID=node-1 \
    cargo run --features server --quiet 2>&1 | sed 's/^/[Node-1] /' &
NODE_PIDS+=($!)
sleep 2

# Start Node 2
echo -e "${YELLOW}Starting Node 2...${NC}"
env $DEBUG_LOGS \
NENYA_CLUSTER_SECRET="$CLUSTER_SECRET" \
NENYA_ENABLE_GOSSIP=1 \
NENYA_LISTEN_ADDR=127.0.0.1:8100 \
NENYA_GOSSIP_ADDR=127.0.0.1:8101 \
NENYA_SEED_NODES=127.0.0.1:8081 \
NENYA_NODE_ID=node-2 \
    cargo run --features server --quiet 2>&1 | sed 's/^/[Node-2] /' &
NODE_PIDS+=($!)

# Wait for cluster to be ready
echo ""
echo -e "${YELLOW}Waiting for cluster to converge...${NC}"
sleep 3

# Check cluster health
HEALTH_OK=true
for port in 8080 8090 8100; do
    if curl -s "http://127.0.0.1:$port/health" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ Node at port $port is healthy${NC}"
    else
        echo -e "${RED}✗ Node at port $port is not responding${NC}"
        HEALTH_OK=false
    fi
done

if [ "$HEALTH_OK" = false ]; then
    echo -e "${RED}Cluster failed to start properly${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}╔════════════════════════════════════╗${NC}"
echo -e "${GREEN}║  ✓ Cluster is online and healthy!  ║${NC}"
echo -e "${GREEN}╚════════════════════════════════════╝${NC}"
echo ""

# Start load generator
echo -e "${YELLOW}Starting load generator (sine wave: 75-225 TPS → 100 TPS target)...${NC}"
cargo run --example cluster_load_generator --quiet -- \
    --nodes 127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100 \
    --pattern sine \
    --total-tps 150 \
    --duration 300 \
    --scope "$SCOPE" 2>&1 | sed 's/^/[Load] /' &
NODE_PIDS+=($!)
sleep 2

echo ""
echo -e "${GREEN}✓ Load generator running${NC}"
echo ""

# Launch visualizer
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${YELLOW}Opening dashboard...${NC}"
echo ""
echo -e "Watch for:"
echo -e "  ${GREEN}●${NC} All nodes turn ${GREEN}GREEN${NC} (optimal health)"
echo -e "  ${GREEN}●${NC} Status changes to ${GREEN}OPTIMAL${NC}"
echo -e "  ${GREEN}●${NC} Graph converges to ${YELLOW}100 TPS target line${NC}"
echo -e "  ${GREEN}●${NC} Topology shows all nodes connected"
echo ""
echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""

cargo run --example cluster_visualizer -- \
    --nodes 127.0.0.1:8080,127.0.0.1:8090,127.0.0.1:8100 \
    --scope "$SCOPE"

# When visualizer closes, cleanup will run automatically
