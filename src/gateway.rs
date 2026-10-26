// SPDX-License-Identifier: AGPL-3.0-only
//! The gateway seam: `up` starts it after verification, `down` stops it.
//! The real HTTP gateway lands in the next handoff.
use crate::state::State;
use anyhow::Result;

pub fn start(_state: &State) -> Result<()> {
    Ok(())
}

pub fn stop() -> Result<()> {
    Ok(())
}
