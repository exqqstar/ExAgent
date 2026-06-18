use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tokio::sync::{oneshot, Notify};

use super::op::ThreadRuntimeError;
use crate::types::{ThreadId, TurnId};

#[derive(Clone)]
pub(super) struct TurnReservations {
    state: Arc<Mutex<TurnReservationState>>,
}

impl TurnReservations {
    pub(super) fn new(next_turn_index: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(TurnReservationState {
                next_turn_index,
                active_turn: None,
            })),
        }
    }

    pub(super) fn reserve_next(
        &self,
        thread_id: &ThreadId,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<(TurnId, ActiveRuntimeTurnGuard)> {
        let mut state = self.state.lock().expect("turn reservation mutex poisoned");
        if state.active_turn.is_some() {
            return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
        }
        let turn_id = TurnId::new(format!("turn_{}", state.next_turn_index));
        state.next_turn_index = state.next_turn_index.saturating_add(1);
        state.active_turn = Some(ActiveRuntimeTurnRecord {
            public_turn_id: Some(turn_id.clone()),
            interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
            interrupted,
        });

        Ok((
            turn_id,
            ActiveRuntimeTurnGuard {
                reservations: self.clone(),
            },
        ))
    }

    pub(super) fn reserve_record(
        &self,
        thread_id: &ThreadId,
        public_turn_id: Option<TurnId>,
        interrupt_tx: oneshot::Sender<()>,
        interrupted: Arc<Notify>,
    ) -> Result<ActiveRuntimeTurnGuard> {
        let mut state = self.state.lock().expect("turn reservation mutex poisoned");
        if state.active_turn.is_some() {
            return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
        }
        state.active_turn = Some(ActiveRuntimeTurnRecord {
            public_turn_id,
            interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
            interrupted,
        });

        Ok(ActiveRuntimeTurnGuard {
            reservations: self.clone(),
        })
    }

    pub(super) fn active_turn_id(&self) -> Option<TurnId> {
        self.state.lock().ok().and_then(|state| {
            state
                .active_turn
                .as_ref()
                .and_then(|record| record.public_turn_id.clone())
        })
    }

    pub(super) async fn signal_interrupt(
        &self,
        thread_id: &ThreadId,
        requested_turn_id: Option<&TurnId>,
    ) -> Result<TurnId> {
        let record = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.active_turn.clone())
            .ok_or_else(|| anyhow!("thread has no active turn"))?;
        let public_turn_id =
            record
                .public_turn_id
                .clone()
                .ok_or_else(|| ThreadRuntimeError::TurnRejected {
                    thread_id: thread_id.clone(),
                    reason: "active operation is not interruptible".to_string(),
                })?;
        if let Some(requested_turn_id) = requested_turn_id {
            if requested_turn_id != &public_turn_id {
                return Err(ThreadRuntimeError::TurnRejected {
                    thread_id: thread_id.clone(),
                    reason: format!("active turn is {}", public_turn_id.as_str()),
                }
                .into());
            }
        }

        let did_send_interrupt = record
            .interrupt_tx
            .lock()
            .expect("active turn interrupt mutex poisoned")
            .take()
            .map(|interrupt_tx| interrupt_tx.send(()).is_ok())
            .unwrap_or(false);
        if !did_send_interrupt {
            return Err(ThreadRuntimeError::TurnRejected {
                thread_id: thread_id.clone(),
                reason: "active turn is already interrupting or completed".to_string(),
            }
            .into());
        }
        record.interrupted.notified().await;
        Ok(public_turn_id)
    }
}

struct TurnReservationState {
    next_turn_index: u64,
    active_turn: Option<ActiveRuntimeTurnRecord>,
}

#[derive(Clone)]
struct ActiveRuntimeTurnRecord {
    public_turn_id: Option<TurnId>,
    interrupt_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    interrupted: Arc<Notify>,
}

pub(super) struct ActiveRuntimeTurnGuard {
    reservations: TurnReservations,
}

impl Drop for ActiveRuntimeTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.reservations.state.lock() {
            state.active_turn = None;
        }
    }
}
