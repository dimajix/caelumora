//! What the tests of the model read of an agent's states: as a store
//! holds them, and what their changes replay onto.

use crate::store::Journaled;

/// `state` as a store holds it, and the live state marked stored.
pub(crate) fn stored<T: Journaled>(state: &mut T) -> T {
    let copy = serde_json::from_value(serde_json::to_value(&*state).unwrap()).unwrap();
    state.mark_stored();
    copy
}

/// Replays the changes of `state` onto `copy` - twice, since replaying
/// is idempotent - and asserts that both are the same afterwards.
pub(crate) fn replayed<T: Journaled>(state: &T, copy: &mut T) {
    for _ in 0..2 {
        for change in state.changes() {
            copy.apply(change).unwrap();
        }
    }
    assert_eq!(serde_json::to_value(&*copy).unwrap(), serde_json::to_value(state).unwrap());
    // The small parts are told whole every time; the growing ones must
    // have recorded nothing: as a copy loaded afresh.
    let reloaded: T = serde_json::from_value(serde_json::to_value(&*copy).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(copy.changes()).unwrap(), serde_json::to_value(reloaded.changes()).unwrap(), "replaying records nothing");
}
