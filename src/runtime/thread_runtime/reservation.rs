use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::{oneshot, Notify};

use super::op::ThreadRuntimeError;
use crate::types::{ThreadId, TurnId};

pub(super) fn reserve_next_turn_from_state(
    turn_reservation: &Arc<Mutex<TurnReservationState>>,
    thread_id: &ThreadId,
    interrupt_tx: oneshot::Sender<()>,
    interrupted: Arc<Notify>,
) -> Result<(TurnId, ActiveRuntimeTurnGuard)> {
    let mut state = turn_reservation
        .lock()
        .expect("turn reservation mutex poisoned");
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
            turn_reservation: turn_reservation.clone(),
        },
    ))
}

pub(super) fn reserve_turn_record_from_state(
    turn_reservation: &Arc<Mutex<TurnReservationState>>,
    thread_id: &ThreadId,
    public_turn_id: Option<TurnId>,
    interrupt_tx: oneshot::Sender<()>,
    interrupted: Arc<Notify>,
) -> Result<ActiveRuntimeTurnGuard> {
    let mut state = turn_reservation
        .lock()
        .expect("turn reservation mutex poisoned");
    if state.active_turn.is_some() {
        return Err(ThreadRuntimeError::ThreadBusy(thread_id.clone()).into());
    }
    state.active_turn = Some(ActiveRuntimeTurnRecord {
        public_turn_id,
        interrupt_tx: Arc::new(Mutex::new(Some(interrupt_tx))),
        interrupted,
    });

    Ok(ActiveRuntimeTurnGuard {
        turn_reservation: turn_reservation.clone(),
    })
}

pub(super) struct TurnReservationState {
    pub(super) next_turn_index: u64,
    pub(super) active_turn: Option<ActiveRuntimeTurnRecord>,
}

#[derive(Clone)]
pub(super) struct ActiveRuntimeTurnRecord {
    pub(super) public_turn_id: Option<TurnId>,
    pub(super) interrupt_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    pub(super) interrupted: Arc<Notify>,
}

pub(super) struct ActiveRuntimeTurnGuard {
    turn_reservation: Arc<Mutex<TurnReservationState>>,
}

impl Drop for ActiveRuntimeTurnGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.turn_reservation.lock() {
            state.active_turn = None;
        }
    }
}
