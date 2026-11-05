use super::*;
use crate::store::search::SearchRequest;

pub(super) async fn search(Query(request): Query<SearchRequest>) -> ApiResult {
    let query = request.prepare().map_err(bad_request)?;
    tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        Ok(Json(json!(store.search_chat_messages(&query)?)))
    })
    .await
    .map_err(|err| ApiError::from(anyhow!("Search task stopped: {err}")))?
}

pub(super) fn spawn_backfill(state: AppState) {
    tokio::spawn(async move {
        loop {
            if state.stopping.load(Ordering::SeqCst) {
                return;
            }
            let gate = state.data_dir_gate.lock().await;
            if state.data_dir_move_in_progress.load(Ordering::SeqCst) {
                drop(gate);
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let result =
                tokio::task::spawn_blocking(|| Store::open()?.backfill_chat_search()).await;
            drop(gate);
            let pause = match result {
                Ok(Ok(false)) => return,
                Ok(Ok(true)) => Duration::from_millis(10),
                Ok(Err(error)) => {
                    eprintln!("orx up: could not index older chats: {error}");
                    Duration::from_secs(60)
                }
                Err(error) => {
                    eprintln!("orx up: search indexing task stopped: {error}");
                    Duration::from_secs(60)
                }
            };
            tokio::time::sleep(pause).await;
        }
    });
}
