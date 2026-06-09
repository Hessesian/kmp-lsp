use crate::indexer::ProgressReporter;
use tower_lsp::lsp_types::*;
use tower_lsp::Client;

/// `$/progress` notification — reports workspace indexing status to the editor.
pub(crate) enum KotlinProgress {}
impl tower_lsp::lsp_types::notification::Notification for KotlinProgress {
    type Params = ProgressParams;
    const METHOD: &'static str = "$/progress";
}

/// Sends LSP `$/progress` notifications via `tower_lsp::Client`.
pub(crate) struct LspProgressReporter(pub(crate) Client);

impl ProgressReporter for LspProgressReporter {
    async fn begin(&self, token: &NumberOrString, message: &str) {
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            self.0
                .send_request::<tower_lsp::lsp_types::request::WorkDoneProgressCreate>(
                    WorkDoneProgressCreateParams {
                        token: token.clone(),
                    },
                ),
        )
        .await;
        self.0
            .send_notification::<KotlinProgress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                    WorkDoneProgressBegin {
                        title: "kmp-lsp".into(),
                        cancellable: Some(false),
                        message: Some(message.to_owned()),
                        percentage: Some(0),
                    },
                )),
            })
            .await;
    }

    async fn report(&self, token: &NumberOrString, done: usize, total: usize) {
        let pct = ((done * 100).checked_div(total).unwrap_or(0)) as u32;
        self.0
            .send_notification::<KotlinProgress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                    WorkDoneProgressReport {
                        cancellable: Some(false),
                        message: Some(format!("{done}/{total} files…")),
                        percentage: Some(pct),
                    },
                )),
            })
            .await;
    }

    /// Send the WorkDone End notification.
    #[allow(dead_code)]
    async fn end(&self, token: &NumberOrString, message: &str) {
        self.0
            .send_notification::<KotlinProgress>(ProgressParams {
                token: token.clone(),
                value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(message.to_owned()),
                })),
            })
            .await;
    }
}
