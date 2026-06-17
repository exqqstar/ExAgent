use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::model::{image_input::load_local_image_for_prompt, multimodal};
use crate::registry::ToolContext;
use crate::tools::{ToolCapabilities, ToolHandler, ToolInvocation, ToolOutcome, ToolSpec};
use crate::types::{ConversationContentPart, ImageDetail, ToolResult, ToolStatus};
use crate::workspace::{resolve_workspace_path, ResolvedWorkspacePath};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ViewImageArgs {
    /// Path to a local image in the workspace.
    pub path: String,
    /// Image detail to send to the model. Defaults to high.
    pub detail: Option<ImageDetail>,
}

pub struct ViewImageTool;

#[async_trait]
impl ToolHandler for ViewImageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            "view_image",
            "View a local image from the workspace and make it available to the model",
            serde_json::to_value(schemars::schema_for!(ViewImageArgs)).unwrap(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities::read_only()
    }

    async fn handle(&self, invocation: ToolInvocation, ctx: &ToolContext) -> ToolOutcome {
        let call = invocation.call;
        let args = match serde_json::from_value::<ViewImageArgs>(call.arguments) {
            Ok(args) => args,
            Err(err) => return view_image_error(call.id, call.name, err.to_string()),
        };

        match view_image(ctx, &args) {
            Ok(viewed) => ToolOutcome::from_result(ToolResult {
                tool_call_id: call.id,
                tool_name: call.name,
                status: ToolStatus::Success,
                content: format!(
                    "Viewed image {} ({}x{}, {})",
                    viewed.display_path, viewed.width, viewed.height, viewed.mime
                ),
                meta: Some(json!({
                    "path": viewed.canonical_path,
                    "requested_path": args.path,
                    "detail": viewed.detail,
                    "width": viewed.width,
                    "height": viewed.height,
                    "mime": viewed.mime,
                })),
                parts: vec![ConversationContentPart::LocalImage {
                    path: viewed.canonical_path,
                    detail: Some(viewed.detail),
                }],
            }),
            Err(err) => view_image_error(call.id, call.name, err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewedImage {
    canonical_path: PathBuf,
    display_path: String,
    detail: ImageDetail,
    width: u32,
    height: u32,
    mime: String,
}

fn view_image(ctx: &ToolContext, args: &ViewImageArgs) -> Result<ViewedImage, String> {
    if !multimodal::supports_images(&ctx.config.model.capabilities.input_modalities) {
        return Err("selected model does not support image input".to_string());
    }

    let detail = args.detail.unwrap_or_default();
    let resolved = resolve_workspace_path(&ctx.config.workspace_root, &args.path)
        .map_err(|err| err.to_string())?;
    let encoded = load_local_image_for_prompt(&resolved.canonical_path, detail)
        .map_err(|err| err.to_string())?;
    let display_path = workspace_display_path(&ctx.config.workspace_root, &resolved);

    Ok(ViewedImage {
        canonical_path: resolved.canonical_path,
        display_path,
        detail,
        width: encoded.width,
        height: encoded.height,
        mime: encoded.mime,
    })
}

fn workspace_display_path(workspace_root: &Path, resolved: &ResolvedWorkspacePath) -> String {
    let workspace_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    resolved
        .canonical_path
        .strip_prefix(&workspace_root)
        .unwrap_or(&resolved.canonical_path)
        .display()
        .to_string()
}

fn view_image_error(tool_call_id: String, tool_name: String, content: String) -> ToolOutcome {
    ToolOutcome::from_result(ToolResult {
        tool_call_id,
        tool_name,
        status: ToolStatus::Error,
        content,
        meta: None,
        parts: Vec::new(),
    })
}
