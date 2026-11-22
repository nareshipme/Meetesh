//! Which model each job uses. Pure rules, so they are tested without files or a window.
//!
//! There are three jobs ("slots"), each with its own choice:
//! - **speech**: turning audio into text (`Role::Asr`)
//! - **hints**: live hints, automatic hints and live points (`Role::Assist`)
//! - **notes**: summary, decisions, action items and titles (`Role::Summary`). Writing notes is a
//!   background job, so it may use a slower, better model than the hints do. By default it
//!   simply uses the hints model.
//!
//! A stored choice that no longer makes sense (the model was removed from the manifest, or it is
//! the wrong kind for the slot) is ignored and the default applies.

use super::{Manifest, ModelEntry, Profile, Role};

/// What kind of model a slot needs: notes and hints are both written by a language model.
pub fn kind_of(slot: Role) -> Role {
    match slot {
        Role::Asr => Role::Asr,
        Role::Assist | Role::Summary => Role::Assist,
    }
}

/// The model a slot uses: the stored choice if it is valid, else the default. For the notes slot
/// the default is whatever the hints slot uses.
pub fn resolve<'a>(manifest: &'a Manifest, profile: Profile, slot: Role, stored: impl Fn(Role) -> Option<String>) -> Option<&'a ModelEntry> {
    let valid = |id: &str| manifest.get(id).filter(|m| m.role == kind_of(slot));
    if let Some(m) = stored(slot).as_deref().and_then(valid) {
        return Some(m);
    }
    match slot {
        Role::Summary => resolve(manifest, profile, Role::Assist, stored),
        _ => manifest.default_for(profile, slot),
    }
}

/// Models that can be chosen for a slot on this device, in manifest order. A device with no
/// default for the kind (assist on mobile) is offered none.
pub fn choices<'a>(manifest: &'a Manifest, profile: Profile, slot: Role) -> Vec<&'a ModelEntry> {
    let kind = kind_of(slot);
    if manifest.default_for(profile, kind).is_none() {
        return vec![];
    }
    manifest.by_role(kind).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn stored(pairs: &[(Role, &str)]) -> impl Fn(Role) -> Option<String> {
        let m: HashMap<Role, String> = pairs.iter().map(|(r, id)| (*r, id.to_string())).collect();
        move |r| m.get(&r).cloned()
    }

    #[test]
    fn defaults_apply_when_nothing_is_stored() {
        let m = Manifest::bundled();
        let none = stored(&[]);
        assert_eq!(resolve(&m, Profile::Desktop, Role::Asr, &none).unwrap().id, "whisper-base.en");
        assert_eq!(resolve(&m, Profile::Desktop, Role::Assist, &none).unwrap().id, "gemma-4-e2b-it");
        assert_eq!(resolve(&m, Profile::Desktop, Role::Summary, &none).unwrap().id, "gemma-4-e2b-it", "notes follow the hints model");
        assert!(resolve(&m, Profile::Mobile, Role::Assist, &none).is_none());
    }

    #[test]
    fn a_valid_choice_wins_and_notes_follow_hints_until_chosen_separately() {
        let m = Manifest::bundled();
        let other = m.by_role(Role::Assist).map(|e| e.id.clone()).find(|id| id != "gemma-4-e2b-it").expect("the manifest offers another language model");
        let s = stored(&[(Role::Assist, &other)]);
        assert_eq!(resolve(&m, Profile::Desktop, Role::Assist, &s).unwrap().id, other);
        assert_eq!(resolve(&m, Profile::Desktop, Role::Summary, &s).unwrap().id, other, "notes follow the hints model");
        let s = stored(&[(Role::Assist, &other), (Role::Summary, "gemma-4-e2b-it")]);
        assert_eq!(resolve(&m, Profile::Desktop, Role::Summary, &s).unwrap().id, "gemma-4-e2b-it", "an explicit notes choice wins");
        assert_eq!(resolve(&m, Profile::Desktop, Role::Assist, &s).unwrap().id, other);
    }

    #[test]
    fn a_stale_or_wrong_kind_choice_is_ignored() {
        let m = Manifest::bundled();
        let s = stored(&[(Role::Asr, "removed-model"), (Role::Assist, "whisper-base.en"), (Role::Summary, "whisper-tiny.en")]);
        assert_eq!(resolve(&m, Profile::Desktop, Role::Asr, &s).unwrap().id, "whisper-base.en", "unknown id");
        assert_eq!(resolve(&m, Profile::Desktop, Role::Assist, &s).unwrap().id, "gemma-4-e2b-it", "a speech model cannot write hints");
        assert_eq!(resolve(&m, Profile::Desktop, Role::Summary, &s).unwrap().id, "gemma-4-e2b-it");
    }

    #[test]
    fn choices_are_the_models_of_the_right_kind() {
        let m = Manifest::bundled();
        let ids = |slot| choices(&m, Profile::Desktop, slot).iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert!(ids(Role::Asr).len() >= 3 && ids(Role::Asr).iter().all(|i| i.starts_with("whisper")));
        assert_eq!(ids(Role::Assist), ids(Role::Summary), "the same language models serve hints and notes");
        assert!(ids(Role::Assist).len() >= 2);
        assert!(choices(&m, Profile::Mobile, Role::Assist).is_empty());
        assert!(!choices(&m, Profile::Mobile, Role::Asr).is_empty());
    }
}
