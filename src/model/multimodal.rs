use anyhow::{anyhow, Result};

use crate::model::image_input::load_local_image_for_prompt;
use crate::types::{ConversationContentPart, ConversationMessage, InputModality, UserInput};

pub const IMAGE_OMITTED_PLACEHOLDER: &str = "[image omitted: selected model has no image input]";

pub fn supports_images(input_modalities: &[InputModality]) -> bool {
    input_modalities.contains(&InputModality::Image)
}

pub fn contains_image(parts: &[ConversationContentPart]) -> bool {
    parts.iter().any(ConversationContentPart::is_image)
}

pub fn input_contains_image(input: &[UserInput]) -> bool {
    input.iter().any(UserInput::is_image)
}

pub fn validate_turn_input_modalities(
    input: &[UserInput],
    input_modalities: &[InputModality],
) -> Result<()> {
    if input_contains_image(input) && !supports_images(input_modalities) {
        return Err(anyhow!("selected model does not support image input"));
    }
    Ok(())
}

pub fn validate_local_image_inputs(input: &[UserInput]) -> Result<()> {
    for item in input {
        if let UserInput::LocalImage { path, detail } = item {
            load_local_image_for_prompt(path, detail.unwrap_or_default())?;
        }
    }
    Ok(())
}

pub fn strip_images_in_place(parts: &mut Vec<ConversationContentPart>) {
    for part in parts.iter_mut() {
        if part.is_image() {
            *part = ConversationContentPart::Text {
                text: IMAGE_OMITTED_PLACEHOLDER.to_string(),
            };
        }
    }
}

pub fn strip_images_from_messages(messages: &mut [ConversationMessage]) {
    for message in messages {
        if contains_image(&message.parts) {
            strip_images_in_place(&mut message.parts);
            message.sync_content_from_parts();
        }
    }
}
