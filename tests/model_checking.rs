//! Model checking of the aggregation/membership state machine
//! (Milestone 4.5).
//!
//! Stateright exhaustively explores every interleaving of publish, deliver,
//! drop, crash, and clock-tick actions for a small cluster and asserts
//! safety invariants on the **real** `gossip::aggregate` code in every
//! reachable state — coverage no finite set of simulation seeds provides.
//! The simulator answers quantitative questions (how fast, how much
//! overshoot); this answers "is the discrete bookkeeping ever wrong".
//!
//! Model shape: one observer node holds receipt records for two peers.
//! Peers publish (at most one message in flight each, keeping the state
//! space finite), messages are delivered or lost, peers crash silently, and
//! time advances in whole ticks. Ages are ticks-since-receipt on the
//! observer's clock — the same age-at-receipt scheme `GossipManager` uses.
//! Time is bounded at HORIZON ticks, comfortably past the stale timeout so
//! every decay phase (fresh, decaying, dropped) is reachable.

#![cfg(feature = "sim")]

use std::time::Duration;

use nenya::gossip::aggregate::{aggregate_peer_rates, staleness_weight, PeerObservation};
use stateright::{Checker, Model, Property};

const N_PEERS: usize = 2;
/// Published rate per peer (distinct values so a double-count is visible in
/// the sum, not just the peer count)
const RATES: [f64; N_PEERS] = [10.0, 20.0];
/// One tick = one sync interval
const SYNC: Duration = Duration::from_secs(1);
/// Stale timeout = 4 ticks: full weight ≤ 2 ticks, decay at 3, dropped at 4
const STALE: Duration = Duration::from_secs(4);
const STALE_TICKS: u8 = 4;
/// Bounded exploration horizon: past STALE so records age through every
/// phase, small enough for exhaustive search (~hundreds of thousands of
/// states)
const HORIZON: u8 = 7;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct State {
    time: u8,
    alive: [bool; N_PEERS],
    /// In-flight message per peer (there is at most one: a peer republishes
    /// only after the previous message resolves)
    inflight: [bool; N_PEERS],
    /// Observer's receipt record per peer: tick of last delivery
    received_at: [Option<u8>; N_PEERS],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Action {
    Tick,
    Publish(usize),
    Deliver(usize),
    Lose(usize),
    Crash(usize),
}

struct AggregationModel;

fn observations(state: &State) -> Vec<PeerObservation> {
    (0..N_PEERS)
        .filter_map(|p| {
            state.received_at[p].map(|at| PeerObservation {
                node_id: format!("peer{}", p),
                age: Duration::from_secs((state.time - at) as u64),
                scope_rates: [("s".to_string(), RATES[p])].into_iter().collect(),
            })
        })
        .collect()
}

impl Model for AggregationModel {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<Self::State> {
        vec![State {
            time: 0,
            alive: [true; N_PEERS],
            inflight: [false; N_PEERS],
            received_at: [None; N_PEERS],
        }]
    }

    fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
        if state.time < HORIZON {
            actions.push(Action::Tick);
        }
        for p in 0..N_PEERS {
            if state.alive[p] && !state.inflight[p] {
                actions.push(Action::Publish(p));
            }
            if state.inflight[p] {
                actions.push(Action::Deliver(p));
                actions.push(Action::Lose(p));
            }
            if state.alive[p] {
                actions.push(Action::Crash(p));
            }
        }
    }

    fn next_state(&self, last: &Self::State, action: Self::Action) -> Option<Self::State> {
        let mut next = last.clone();
        match action {
            Action::Tick => next.time += 1,
            Action::Publish(p) => next.inflight[p] = true,
            Action::Deliver(p) => {
                next.inflight[p] = false;
                next.received_at[p] = Some(next.time);
            }
            Action::Lose(p) => next.inflight[p] = false,
            Action::Crash(p) => {
                next.alive[p] = false;
                // A crashed peer's in-flight message may still arrive;
                // leaving it in flight models exactly that
            }
        }
        Some(next)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            // A peer whose record is stale_timeout old contributes nothing:
            // no phantom load in ANY reachable state, and the live peer
            // count includes exactly the fresh records
            Property::<Self>::always("no phantom load", |_, state| {
                let obs = observations(state);
                let agg = aggregate_peer_rates(&obs, SYNC, STALE);
                let fresh: Vec<&PeerObservation> = obs.iter().filter(|o| o.age < STALE).collect();
                let fresh_sum: f64 = fresh.iter().map(|o| o.scope_rates["s"]).sum();
                let external = agg.scope_rates.get("s").copied().unwrap_or(0.0);
                agg.live_peers == fresh.len() && external >= 0.0 && external <= fresh_sum + 1e-9
            }),
            // Each peer is counted exactly once, with exactly the decay
            // weight its age implies (cross-checked against the weight
            // function peer by peer — a double-add or a missed peer breaks
            // the equality)
            Property::<Self>::always("each peer counted once", |_, state| {
                let obs = observations(state);
                let agg = aggregate_peer_rates(&obs, SYNC, STALE);
                let expected: f64 = obs
                    .iter()
                    .map(|o| staleness_weight(o.age, SYNC, STALE) * o.scope_rates["s"])
                    .sum();
                let external = agg.scope_rates.get("s").copied().unwrap_or(0.0);
                (external - expected).abs() < 1e-9
            }),
            // After quiescence (nothing in flight, every live peer's record
            // fresh, every crashed peer silent past the stale timeout) the
            // live peer count equals exactly the set of live peers
            Property::<Self>::always("num_peers correct after quiescence", |_, state| {
                let quiescent = (0..N_PEERS).all(|p| {
                    !state.inflight[p]
                        && if state.alive[p] {
                            // live peer: record present and within the
                            // full-weight window
                            state.received_at[p].is_some_and(|at| state.time - at <= 2)
                        } else {
                            // crashed peer: silent at least a full stale
                            // timeout
                            state.received_at[p].is_none_or(|at| state.time - at >= STALE_TICKS)
                        }
                });
                if !quiescent {
                    return true;
                }
                let agg = aggregate_peer_rates(&observations(state), SYNC, STALE);
                agg.live_peers == state.alive.iter().filter(|a| **a).count()
            }),
        ]
    }
}

#[test]
fn model_check_aggregation_invariants() {
    let checker = AggregationModel.checker().spawn_bfs().join();
    checker.assert_properties();
    // Exhaustive within the horizon — make sure the search actually covered
    // the space rather than terminating trivially. The full reachable space
    // for these bounds is ~4.5k unique states (8 time values × 2² liveness
    // × 2² in-flight × up to 9² receipt-record combinations, less
    // unreachable mixes).
    assert!(
        checker.unique_state_count() > 4_000,
        "state space unexpectedly small: {}",
        checker.unique_state_count()
    );
}
