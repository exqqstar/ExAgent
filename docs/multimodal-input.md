# Structured Multimodal Input

ExAgent supports structured text and image input for desktop turns. The first
shipping scope is photo attachments from local files, plus provider adapter
support for image URL/data URL parts. Arbitrary document/file ingestion,
Chrome attachment, and Plugins are separate follow-up tracks.

## User Flow

In the desktop composer, `Add photos` opens the native file picker with image
formats enabled. Users can also paste an image from the clipboard or drop image
files anywhere onto the window. All three paths import images into the app
attachment cache, then render compact thumbnail chips with a remove action. A
prompt can be sent with text plus images, or images alone.

Drag/drop is delivered through Tauri's webview drag-drop event, which provides
absolute file paths and reuses the same path-import command as the picker. Paste
is the only route that ships raw bytes across the IPC boundary.

If the selected model is known to accept text only, the composer disables new
photo selection, shows a visible warning for already selected photos, and
disables Send until the user removes the photos or switches models. The runtime
keeps the same validation server-side so direct protocol clients cannot bypass
the check.

Historical user messages render attached images from the structured turn input.
Image containers use stable aspect ratios and fall back to a neutral placeholder
if the local file is no longer available.

## Core Data Model

`ConversationMessage.content` remains a text preview for compatibility.
Structured data lives in `ConversationMessage.parts`.

Turn input is represented by `UserInput`:

- `text`
- `local_image`
- `image_url`

Rollouts store local image paths and detail metadata only. Desktop-selected
images are first copied into the app attachment cache so previews and provider
requests do not depend on the original file remaining in place. Rollouts do not
store base64 image bytes. Local image bytes are loaded and encoded only when
building provider requests.

## Attachment Cache Lifecycle

The MVP intentionally does not automatically delete attachment cache files.
Cached images are retained so historical thread previews, replay, rollback, and
fork flows can still resolve local image paths.

If a cached file is removed by the user, OS cleanup, or a future manual cache
clear action, provider adapters degrade the missing image to a clear text
placeholder instead of crashing the turn.

## Known Limitations

- Supported desktop attachment formats are PNG, JPEG, WebP, and GIF, matching
  the picker filter.
- Pasted byte imports are capped at 20 MiB per image, at most 8 images per
  batch, and 20 MiB total per pasted batch before the frontend sends IPC.
- Path imports from the picker or native drag/drop are checked by the desktop
  cache command before copying into the app cache.
- A paste containing both text and an image attaches the image and suppresses
  the text insertion.
- A multi-image import batch is all-or-nothing: one unreadable file fails the
  whole batch with a visible warning.

## Model Gating

Model capabilities include `input_modalities`.

ExAgent uses two layers:

- Layer 1 rejects newly submitted image input before recording a turn when the
  current model is known text-only.
- Layer 2 strips historical image parts to a text placeholder when prompt or
  compaction views are built for a text-only model.

Unknown models default to `[text, image]`. If an unknown provider rejects image
input, the provider error is surfaced as a normal runtime error.

## Image Encoding

`src/model/image_input.rs` owns local image validation and encoding:

- PNG, JPEG, WebP, and decodeable GIF input are accepted.
- Source PNG/JPEG/WebP bytes are preserved when they already fit the configured
  size policy.
- Oversized images are resized before encoding.
- Encoded data URLs are cached by content hash and detail setting.
- Missing or unreadable historical files degrade to text in provider adapters.

## Provider Serialization

Adapters serialize structured user message parts at the provider boundary:

- OpenAI-compatible Chat Completions: text and `image_url` content blocks.
- ChatGPT Codex Responses API: `input_text` and `input_image` blocks.
- Anthropic Messages: text blocks plus base64 or URL image sources.
- Gemini Generate Content: text parts plus `inlineData` or `fileData`.

Adapters do not decide whether a model supports images. They receive the prompt
view that runtime already gated or stripped.

## Protocol Shape

`TurnStartParams` keeps legacy `prompt` and adds optional `input`.

Legacy clients can keep sending:

```json
{
  "thread_id": "session_...",
  "prompt": "Summarize this."
}
```

Multimodal clients can send:

```json
{
  "thread_id": "session_...",
  "prompt": "Describe this screenshot.",
  "input": [
    { "type": "text", "text": "Describe this screenshot." },
    { "type": "local_image", "path": "/Users/me/Desktop/screen.png", "detail": "high" }
  ]
}
```

When `input` is present, it is the authoritative user input. `prompt` remains a
compatibility preview for clients and logs.
