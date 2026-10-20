//! `AbortController` y `AbortSignal` (DOM §3), básicos: un signal es un `EventTarget` no-nodo
//! con su estado; "signal abort" corre los abort algorithms y dispara `abort`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::events::{EventInit, EventKind, EventTarget, ScriptHost};
use crate::{Document, DomException};

/// La razón de un abort: un valor de script (handle) o un `DOMException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    Script(u64),
    Exception(DomException),
}

/// Un abort algorithm nativo.
pub type AbortAlgorithm = Arc<dyn Fn(&mut Document) + Send + Sync>;

#[derive(Clone, Default)]
struct SignalState {
    reason: Option<AbortReason>,
    algorithms: Vec<AbortAlgorithm>,
}

impl std::fmt::Debug for SignalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignalState")
            .field("reason", &self.reason)
            .field("algorithms", &self.algorithms.len())
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AbortState {
    signals: HashMap<EventTarget, SignalState>,
}

/// `AbortController`: su `signal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbortController {
    pub signal: EventTarget,
}

impl Document {
    /// `new AbortController()`.
    pub fn create_abort_controller(&mut self) -> AbortController {
        let signal = self.create_event_target();
        self.aborts.signals.insert(signal, SignalState::default());
        AbortController { signal }
    }

    /// `AbortSignal.aborted`.
    pub fn signal_aborted(&self, signal: EventTarget) -> bool {
        self.aborts
            .signals
            .get(&signal)
            .is_some_and(|s| s.reason.is_some())
    }

    /// `AbortSignal.reason`.
    pub fn signal_reason(&self, signal: EventTarget) -> Option<AbortReason> {
        self.aborts.signals.get(&signal).and_then(|s| s.reason)
    }

    /// `AbortSignal.throwIfAborted()`.
    pub fn throw_if_aborted(&self, signal: EventTarget) -> Result<(), AbortReason> {
        match self.signal_reason(signal) {
            Some(r) => Err(r),
            None => Ok(()),
        }
    }

    /// Agrega un abort algorithm (se ignora si el signal ya abortó).
    pub fn add_abort_algorithm(&mut self, signal: EventTarget, algorithm: AbortAlgorithm) {
        if let Some(s) = self.aborts.signals.get_mut(&signal)
            && s.reason.is_none()
        {
            s.algorithms.push(algorithm);
        }
    }

    /// "Signal abort" (§3.2): `controller.abort(reason)`; sin razón, un `AbortError`.
    pub fn signal_abort(
        &mut self,
        signal: EventTarget,
        reason: Option<AbortReason>,
        host: &mut dyn ScriptHost,
    ) {
        let Some(state) = self.aborts.signals.get_mut(&signal) else {
            return;
        };
        if state.reason.is_some() {
            return;
        }
        state.reason = Some(reason.unwrap_or(AbortReason::Exception(DomException::AbortError)));
        let algorithms = std::mem::take(&mut state.algorithms);
        for a in algorithms {
            a(self);
        }
        self.fire_event(signal, "abort", EventInit::default(), EventKind::Event, host);
    }

    /// `AbortSignal.abort(reason)`: un signal ya abortado.
    pub fn aborted_signal(&mut self, reason: Option<AbortReason>) -> EventTarget {
        let c = self.create_abort_controller();
        if let Some(s) = self.aborts.signals.get_mut(&c.signal) {
            s.reason = Some(reason.unwrap_or(AbortReason::Exception(DomException::AbortError)));
        }
        c.signal
    }
}
