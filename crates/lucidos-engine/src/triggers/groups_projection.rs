use std::collections::HashMap;
use std::ops::Deref;

use super::groups::{TriggerGroup, TriggerGroupEventRow};

#[derive(Clone, Debug)]
struct Versioned<T> {
    seq: i64,
    value: T,
}

fn retain_newest<T>(current: &mut Option<Versioned<T>>, incoming: Option<Versioned<T>>) {
    if let Some(incoming) = incoming {
        if current.as_ref().is_none_or(|old| old.seq < incoming.seq) {
            *current = Some(incoming);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct GroupState {
    creation: Option<Versioned<TriggerGroup>>,
    name: Option<Versioned<String>>,
    order: Option<Versioned<i32>>,
    deletion: Option<Versioned<()>>,
}

impl GroupState {
    fn visible(&self) -> Option<TriggerGroup> {
        let creation = self.creation.as_ref()?;
        if self
            .deletion
            .as_ref()
            .is_some_and(|deleted| deleted.seq > creation.seq)
        {
            return None;
        }
        let mut group = creation.value.clone();
        if let Some(name) = self.name.as_ref().filter(|name| name.seq > creation.seq) {
            group.name = name.value.clone();
        }
        if let Some(order) = self.order.as_ref().filter(|order| order.seq > creation.seq) {
            group.order = order.value;
        }
        Some(group)
    }
}

/// INVARIANT: A read-only group map backed by bounded, sequence-versioned lifecycle state.
/// Each operation retains its own version so independent fields can arrive
/// out of order. Deleted groups retain tombstones until rebuilt from events.
#[derive(Clone, Debug, Default)]
pub struct TriggerGroupProjection {
    visible: HashMap<String, TriggerGroup>,
    states: HashMap<String, GroupState>,
}

impl Deref for TriggerGroupProjection {
    type Target = HashMap<String, TriggerGroup>;

    fn deref(&self) -> &Self::Target {
        &self.visible
    }
}

impl TriggerGroupProjection {
    pub fn apply(&mut self, row: &TriggerGroupEventRow) {
        let Some(id) = row
            .payload
            .get("group_id")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut incoming = GroupState::default();
        match row.event_type.as_str() {
            "TriggerGroupCreated" => {
                match TriggerGroup::from_created_payload(&row.payload, row.created) {
                    Ok(group) => {
                        incoming.creation = Some(Versioned {
                            seq: row.seq,
                            value: group,
                        })
                    }
                    Err(error) => {
                        crate::log_legacy!(
                            "[TriggerReplay] TriggerGroupCreated payload for {} did not parse: {}",
                            id,
                            error
                        );
                        return;
                    }
                }
            }
            "TriggerGroupRenamed" => {
                incoming.name = row
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|name| Versioned {
                        seq: row.seq,
                        value: name.to_owned(),
                    });
            }
            "TriggerGroupReordered" => {
                incoming.order = row
                    .payload
                    .get("order")
                    .and_then(|v| v.as_i64())
                    .map(|order| Versioned {
                        seq: row.seq,
                        value: order as i32,
                    });
            }
            "TriggerGroupDeleted" => {
                incoming.deletion = Some(Versioned {
                    seq: row.seq,
                    value: (),
                })
            }
            _ => return,
        }
        self.merge_group(id, incoming, false);
    }

    pub fn merge(&mut self, snapshot: Self) {
        for (id, state) in snapshot.states {
            self.merge_group(&id, state, true);
        }
    }

    fn merge_group(&mut self, id: &str, incoming: GroupState, persisted_snapshot: bool) {
        let state = self.states.entry(id.to_owned()).or_default();
        if persisted_snapshot {
            if let (Some(current), Some(loaded)) = (&mut state.creation, &incoming.creation) {
                if current.seq == loaded.seq {
                    current.value.created = loaded.value.created;
                }
            }
        }
        retain_newest(&mut state.creation, incoming.creation);
        retain_newest(&mut state.name, incoming.name);
        retain_newest(&mut state.order, incoming.order);
        retain_newest(&mut state.deletion, incoming.deletion);
        match state.visible() {
            Some(group) => {
                self.visible.insert(id.to_owned(), group);
            }
            None => {
                self.visible.remove(id);
            }
        }
    }
}

#[cfg(test)]
#[path = "groups_projection_tests.rs"]
mod tests;
