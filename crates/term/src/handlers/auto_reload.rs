//! Terminal presentation of shared external-change confirmations.

use std::borrow::Cow;

use view::handlers::auto_reload::{resolve_reload, ReloadDecision, ReloadRequest};

use crate::{
    compositor::Compositor,
    ui::{self, Prompt, PromptEvent},
};

/// Co-Authored-By: Anthony Rubick <68485672+AnthonyMichaelTDM@users.noreply.github.com>
pub(crate) fn prompt_reload_modified(compositor: &mut Compositor, request: ReloadRequest) {
    let display = request.display_name().to_owned();
    let mut request = Some(request);
    let prompt = Prompt::new(
        Cow::Owned(format!("{display} changed externally (unsaved changes exist). Press Enter to reload, Esc to ignore: ")),
        None,
        ui::completers::none,
        move |cx, _input, event| {
            let decision = match event {
                PromptEvent::Validate => ReloadDecision::Reload,
                PromptEvent::Abort => ReloadDecision::Ignore,
                PromptEvent::Update => return,
            };
            if let Some(request) = request.take() {
                resolve_reload(cx.editor, request, decision);
            }
        },
    );
    compositor.push(Box::new(prompt));
}
