//! Small, read-only previews. The frontend serializes visible requests.
use crate::ipc::{run_blocking,SharedState};
use tauri::State;

#[tauri::command]
pub async fn edit_preview_style(
    state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value,
)->Result<tauri::ipc::Response,String>{
    let recipe=super::recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(),move |_|{
        let source=super::preview::interactive_source(&session_id)?;
        let small=super::photocraft::resize(&source,Some(240))?;
        let scale=small.size.width.max(small.size.height) as f32/source.size.width.max(source.size.height).max(1) as f32;
        let rendered=super::photocraft::render_document_scaled(&small,&recipe,scale)?;
        let (bytes,_)=super::photocraft::jpeg_preview(&rendered,75)?;
        Ok(tauri::ipc::Response::new(bytes))
    }).await
}
