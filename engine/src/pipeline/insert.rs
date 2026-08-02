//! Sealed insert-chain wet switch — in-process DSP host (DAW-grade).
//!
//! Topology: `{bus}.monitor → buschain_fx_{bus} → buschain_post_{bus} → dest`

use anyhow::Result;

use crate::backend::{FilterChainRuntime, AudioBackend};
use crate::clock::GraphClock;
use crate::domain::{ChainEnsureMode, ChainSpec, ChainState, InsertSlot};
use crate::plan::DesiredState;

use super::insert_host;

pub fn ensure_fx_chain(
    _runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    desired: &mut DesiredState,
    clock: &GraphClock,
    spec: &ChainSpec,
    mode: ChainEnsureMode,
    arm_egress: bool,
) -> Result<ChainState> {
    insert_host::ensure_fx_chain(backend, desired, clock, spec, mode, arm_egress)
}

pub fn push_fx_controls(
    _runtime: &mut FilterChainRuntime,
    bus: &str,
    inserts: &[InsertSlot],
) -> Result<()> {
    insert_host::push_fx_controls(bus, inserts)
}

pub fn teardown_fx_chain(
    _runtime: &mut FilterChainRuntime,
    backend: &mut dyn AudioBackend,
    bus: &str,
) -> Result<()> {
    insert_host::teardown_fx_chain(backend, bus)
}

pub fn probe_chain_state(
    _runtime: &mut FilterChainRuntime,
    bus: &str,
    inserts_len: usize,
    dest: &str,
    require_dest: bool,
) -> ChainState {
    insert_host::probe_chain_state(bus, inserts_len, dest, require_dest)
}

