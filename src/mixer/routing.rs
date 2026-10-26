//! Prepared, bounded audio routes between existing named buses.
use super::effects::{self, Change};
use super::queue::{self, Consumer, Producer, PushError};
use super::{Action, BLOCK, BusId, Commands, Mixer, MixerEvent, MixerHandle};
use crate::{Error, ErrorCode, Result};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Total main outputs plus sends in one graph, including omitted default outputs.
pub const MAX_BUS_ROUTES: usize = 1024;
/// Both taps are after inserts and the bus pause fade. Pre-fader excludes bus
/// gain/pan; post-fader includes them. Mute/solo gate both taps and sidechain keys.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SendPosition {
    PreFader,
    #[default]
    PostFader,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusSend {
    pub destination: BusId,
    /// Finite nonnegative linear gain. Routes can sum above full scale.
    pub gain: f32,
    pub position: SendPosition,
}
/// One named bus's complete routing/settings. Omitted buses retain one unity
/// post-fader output to master. The master is a sink, configured through its
/// existing gain/pause/effect APIs, and cannot appear as a source here.
#[derive(Clone, Debug, PartialEq)]
pub struct BusRouting {
    pub bus: BusId,
    /// Main post-fader output, or None for a send-only/detector-only bus.
    pub output: Option<BusId>,
    pub sends: Vec<BusSend>,
    /// Linear stereo balance, -1..=1. Nonzero requires a stereo mixer.
    pub pan: f32,
    /// Gates this bus's outgoing audio/keys; does not freeze voices or effect state.
    pub muted: bool,
    /// Selects audio paths through this bus, including upstream feeds/downstream returns.
    pub solo: bool,
}
impl BusRouting {
    pub fn new(bus: BusId) -> Self {
        Self {
            bus,
            output: Some(BusId::MASTER),
            sends: Vec::new(),
            pan: 0.0,
            muted: false,
            solo: false,
        }
    }
}
/// A successfully queued edit ID. Poll RoutingApplied/Rejected for its outcome.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RoutingId(pub u64);

pub(super) struct Edge {
    pub destination: usize,
    pub gain: f64,
    pub position: SendPosition,
    pub enabled: bool,
}
pub(super) struct Node {
    pub edges: Vec<Edge>,
    pub pan: f64,
    pub muted: bool,
    pub solo: bool,
    pub audible: bool,
}
pub(super) struct Graph {
    pub nodes: Vec<Node>,
    pub incoming: Vec<Vec<usize>>,
    pub input: [bool; 256],
    pub pre: Vec<f32>,
}
impl Graph {
    fn new(settings: &[BusRouting], buses: usize, channels: usize) -> Result<Self> {
        if settings.len() >= buses {
            return Err(Error::invalid("Too many/duplicate routing bus definitions"));
        }
        let routes = settings
            .iter()
            .try_fold(buses - 1 - settings.len(), |n, s| {
                n.checked_add(s.output.is_some() as usize)
                    .and_then(|n| n.checked_add(s.sends.len()))
            })
            .ok_or_else(|| Error::limit("Routing edge count overflow"))?;
        if routes > MAX_BUS_ROUTES {
            return Err(Error::limit("Too many bus routes"));
        }
        let mut graph = Self {
            nodes: (0..buses)
                .map(|b| Node {
                    edges: if b == 0 {
                        Vec::new()
                    } else {
                        vec![Edge {
                            destination: 0,
                            gain: 1.0,
                            position: SendPosition::PostFader,
                            enabled: true,
                        }]
                    },
                    pan: 0.0,
                    muted: false,
                    solo: false,
                    audible: true,
                })
                .collect(),
            incoming: (0..buses).map(|_| Vec::new()).collect(),
            input: [true; 256],
            pre: vec![0.0; BLOCK * channels],
        };
        let mut seen = [false; 256];
        for setting in settings {
            let b = setting.bus.0;
            if b == 0 || b >= buses || seen[b] {
                return Err(Error::invalid(
                    "Routing needs distinct existing named source buses",
                ));
            }
            seen[b] = true;
            if !setting.pan.is_finite()
                || !(-1.0..=1.0).contains(&setting.pan)
                || (channels != 2 && setting.pan != 0.0)
            {
                return Err(Error::invalid(
                    "Bus pan must be -1..=1 and requires stereo when nonzero",
                ));
            }
            let node = &mut graph.nodes[b];
            node.edges.clear();
            node.pan = setting.pan as f64;
            node.muted = setting.muted;
            node.solo = setting.solo;
            let mut destinations = [false; 256];
            for (destination, gain, position) in setting
                .output
                .iter()
                .map(|d| (*d, 1.0, SendPosition::PostFader))
                .chain(
                    setting
                        .sends
                        .iter()
                        .map(|s| (s.destination, s.gain, s.position)),
                )
            {
                let to = destination.0;
                if to >= buses || to == b || destinations[to] {
                    return Err(Error::invalid(
                        "Missing, self or duplicate audio route destination",
                    ));
                }
                if !gain.is_finite() || gain < 0.0 {
                    return Err(Error::invalid("Send gain must be finite and nonnegative"));
                }
                destinations[to] = true;
                node.edges.push(Edge {
                    destination: to,
                    gain: gain as f64,
                    position,
                    enabled: true,
                });
            }
        }
        for (from, node) in graph.nodes.iter().enumerate() {
            for edge in &node.edges {
                graph.incoming[edge.destination].push(from);
            }
        }
        // Named buses are deterministically ordered; master is always last.
        let mut order = [0usize; 256];
        let mut done = [false; 256];
        for entry in &mut order[..buses - 1] {
            let ready = (1..buses)
                .find(|&b| !done[b] && graph.incoming[b].iter().all(|&source| done[source]))
                .ok_or_else(|| Error::invalid("Audio routes contain a cycle"))?;
            *entry = ready;
            done[ready] = true;
        }
        if graph.nodes.iter().any(|n| n.solo) {
            let mut up = [false; 256];
            let mut down = [false; 256];
            for &b in order[..buses].iter().rev() {
                up[b] =
                    graph.nodes[b].solo || graph.nodes[b].edges.iter().any(|e| up[e.destination]);
            }
            for &b in &order[..buses] {
                down[b] |= graph.nodes[b].solo;
                for edge in &graph.nodes[b].edges {
                    down[edge.destination] |= down[b];
                }
            }
            for (b, node) in graph.nodes.iter_mut().enumerate() {
                graph.input[b] = up[b];
                node.audible = up[b] || down[b];
                for edge in &mut node.edges {
                    edge.enabled = down[b] || (up[b] && up[edge.destination]);
                }
            }
        }
        Ok(graph)
    }
}

pub(super) struct Edit {
    id: RoutingId,
    graph: Option<Box<Graph>>,
    event: Option<MixerEvent>,
    credits: Arc<AtomicUsize>,
}
impl Drop for Edit {
    fn drop(&mut self) {
        self.credits.fetch_sub(1, Ordering::Release);
    }
}
pub(super) struct AudioRouting {
    pub graph: Option<Box<Graph>>,
    returns: Producer<Box<Edit>>,
    retired: VecDeque<Box<Edit>>,
}
pub(super) struct ControlRouting {
    returns: Consumer<Box<Edit>>,
    credits: Arc<AtomicUsize>,
    capacity: usize,
    next: u64,
}
pub(super) fn create(capacity: usize) -> (AudioRouting, ControlRouting) {
    let (tx, rx) = queue::queue(capacity);
    (
        AudioRouting {
            graph: None,
            returns: tx,
            retired: VecDeque::with_capacity(capacity),
        },
        ControlRouting {
            returns: rx,
            credits: Arc::new(AtomicUsize::new(0)),
            capacity,
            next: 0,
        },
    )
}
impl AudioRouting {
    pub fn send_returns(&mut self) {
        while let Some(edit) = self.retired.pop_front() {
            match self.returns.push(edit) {
                Ok(()) => (),
                Err(PushError::Full(edit) | PushError::Disconnected(edit)) => {
                    self.retired.push_front(edit);
                    break;
                }
            }
        }
    }
}
impl ControlRouting {
    pub fn poll(&mut self) -> Option<MixerEvent> {
        self.returns.pop()?.event
    }
}
impl MixerHandle {
    /// Replace the named-bus routing graph/settings atomically. Empty settings
    /// restore legacy flat routing. Preparation allocates; render does not.
    /// Audio-only validation is synchronous; actual sidechain dependencies are
    /// revalidated on application. Poll the returned ID's outcome. Each unpolled
    /// edit consumes one credit, up to MixerConfig::queue, including retired data.
    pub fn set_routing(&mut self, settings: &[BusRouting]) -> Result<RoutingId> {
        self.now().set_routing(settings)
    }
}
impl Commands<'_> {
    pub fn set_routing(self, settings: &[BusRouting]) -> Result<RoutingId> {
        let control = &self.handle.routing;
        if control.credits.load(Ordering::Acquire) >= control.capacity {
            return Err(Error::limit(
                "Poll routing events before sending more edits",
            ));
        }
        let next = control
            .next
            .checked_add(1)
            .ok_or_else(|| Error::limit("Routing ID overflow"))?;
        let graph = if settings.is_empty() {
            None
        } else {
            Some(Box::new(Graph::new(
                settings,
                self.handle.buses.len() + 1,
                self.handle.channels,
            )?))
        };
        let id = RoutingId(control.next);
        control.credits.fetch_add(1, Ordering::Relaxed);
        let edit = Box::new(Edit {
            id,
            graph,
            event: None,
            credits: control.credits.clone(),
        });
        self.handle.send(self.frame, Action::Routing(edit))?;
        self.handle.routing.next = next;
        Ok(id)
    }
}
impl Mixer {
    pub(super) fn apply_routing(&mut self, frame: u64, mut edit: Box<Edit>) {
        if let Some(order) = effects::topology(&self.buses, Change::Current, edit.graph.as_deref())
        {
            self.effects.order = order;
            std::mem::swap(&mut self.routing.graph, &mut edit.graph);
            edit.event = Some(MixerEvent::RoutingApplied(edit.id, frame));
        } else {
            edit.event = Some(MixerEvent::RoutingRejected(
                edit.id,
                ErrorCode::InvalidArgument,
            ));
        }
        self.routing.retired.push_back(edit);
    }
}
