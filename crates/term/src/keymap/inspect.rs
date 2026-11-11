//! Descriptions of available actions for pending component prefixes.

use ui_core::keymap::{Binding, Keymap};

pub fn filter<A: Clone>(keys: &Keymap<A>, available: impl Fn(&A) -> bool + Copy) -> Keymap<A> {
    Keymap(
        keys.0
            .iter()
            .filter_map(|(key, binding)| {
                let binding = match binding {
                    Binding::Actions(actions) if actions.iter().all(available) => {
                        Binding::Actions(actions.clone())
                    }
                    Binding::Actions(_) => return None,
                    Binding::Keys(child) => {
                        let child = filter(child, available);
                        if child.0.is_empty() {
                            return None;
                        }
                        Binding::Keys(child)
                    }
                    Binding::Unbound => Binding::Unbound,
                };
                Some((*key, binding))
            })
            .collect(),
    )
}

pub fn prefix_info<A>(keys: &Keymap<A>, description: impl Fn(&A) -> String) -> view::info::Info {
    let mut rows = std::collections::BTreeMap::<String, Vec<String>>::new();
    for (key, binding) in &keys.0 {
        let label = match binding {
            Binding::Actions(actions) if !actions.is_empty() => actions
                .iter()
                .map(&description)
                .collect::<Vec<_>>()
                .join("; "),
            Binding::Keys(_) => "More actions".into(),
            _ => continue,
        };
        rows.entry(label)
            .or_default()
            .push(key.key_sequence_format());
    }
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(label, mut keys)| {
            keys.sort();
            (keys.join(", "), label)
        })
        .collect();
    view::info::Info::new("Actions", &rows)
}
