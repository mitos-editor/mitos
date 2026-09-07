use event::register_hook;
use view::events::DocumentSyntaxRequested;

use crate::job;

pub(super) fn register_hooks() {
    register_hook!(move |event: &mut DocumentSyntaxRequested<'_>| {
        let Some(request) = event.doc.syntax_request() else {
            return Ok(());
        };
        let id = event.doc.id();
        tokio::spawn(async move {
            let Some(result) = request.compute().await else {
                return;
            };
            job::dispatch(move |editor, _| {
                let Some(doc) = editor.document_mut(id) else {
                    return;
                };
                if request.complete(doc, result) {
                    editor.refresh_spelling(id);
                }
            })
            .await;
        });
        Ok(())
    });
}
