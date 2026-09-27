//! Terminal presentation of editor-owned workspace trust requests.

use view::handlers::workspace_trust::{
    dismiss_request, resolve_request, TrustDecision, TrustRequest,
};

use crate::{compositor::Compositor, ui};

const ID: &str = "workspace-trust-select";

pub(crate) fn prompt(editor: &view::Editor, compositor: &mut Compositor, request: TrustRequest) {
    if request.is_current(editor) {
        compositor.replace_or_push(ID, select(request));
    }
}

const TRUST_MESSAGE: &str = "Trust this workspace?

Trusted workspaces may load local Mitos config files (`.mitos/*`) and auto-start language servers. \
Both can execute arbitrary code. Only trust workspaces whose contents you have inspected.";

#[derive(Default, Clone, Copy, Debug)]
pub enum TrustChoice {
    #[default]
    Trust,
    Never,
}

fn select(request: TrustRequest) -> ui::Select<TrustChoice> {
    ui::Select::new(
        TRUST_MESSAGE,
        [TrustChoice::Trust, TrustChoice::Never],
        (),
        move |editor, option, event| {
            let decision = match event {
                ui::PromptEvent::Validate => match option {
                    TrustChoice::Trust => TrustDecision::Trust,
                    TrustChoice::Never => TrustDecision::Exclude,
                },
                ui::PromptEvent::Abort => {
                    dismiss_request(editor, &request);
                    return;
                }
                ui::PromptEvent::Update => return,
            };
            if let Err(err) = resolve_request(editor, &request, decision) {
                editor.set_error(|| err.to_string());
            }
        },
    )
}

impl crate::ui::menu::Item for TrustChoice {
    type Data = ();

    fn format(&self, _data: &Self::Data) -> tui::widgets::Row<'_> {
        tui::widgets::Row::new([match self {
            TrustChoice::Trust => "Trust",
            TrustChoice::Never => "Never",
        }])
    }
}
