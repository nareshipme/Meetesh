//! Renders the chat template stored inside a model file (Hugging Face style Jinja).
//!
//! llama.cpp's built-in renderer only knows a fixed list of template families and fails on newer
//! ones (Gemma 4's, for example). Rendering the model's own template keeps "swap the model by
//! editing the manifest" true without hard-coding any model's prompt format.

use minijinja::{context, Environment, Error as JinjaError, ErrorKind, Value};

use crate::assist::ChatMessage;
use crate::{Error, Result};

/// Render `template` for `messages` with the generation prompt appended (ready to be tokenized).
pub fn render_chat_template(template: &str, messages: &[ChatMessage], bos_token: &str, eos_token: &str) -> Result<String> {
    let mut env = Environment::new();
    // Hugging Face renders chat templates with these two settings.
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    // Python-style helpers templates rely on: .get(), .split(), .strip(), .startswith(), ...
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);
    env.add_function("raise_exception", |msg: String| -> std::result::Result<Value, JinjaError> {
        Err(JinjaError::new(ErrorKind::InvalidOperation, msg))
    });
    env.add_template("chat", template).map_err(|e| Error::Other(format!("chat template did not parse: {e}")))?;

    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| context! { role => m.role.as_str(), content => m.content.as_str() })
        .collect();
    env.get_template("chat")
        .and_then(|t| {
            t.render(context! {
                messages => msgs,
                bos_token => bos_token,
                eos_token => eos_token,
                add_generation_prompt => true,
                enable_thinking => false,
            })
        })
        .map_err(|e| Error::Other(format!("chat template failed to render: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::ChatRole;

    fn conv() -> Vec<ChatMessage> {
        vec![
            ChatMessage::new(ChatRole::System, "  Be brief.  "),
            ChatMessage::new(ChatRole::User, "Hello there"),
        ]
    }

    const CHATML: &str = "{% for m in messages %}<|im_start|>{{ m['role'] }}\n{{ m['content'] }}<|im_end|>\n{% endfor %}{% if add_generation_prompt %}<|im_start|>assistant\n{% endif %}";

    #[test]
    fn renders_a_chatml_style_template() {
        let out = render_chat_template(CHATML, &conv(), "<s>", "</s>").unwrap();
        assert_eq!(out, "<|im_start|>system\n  Be brief.  <|im_end|>\n<|im_start|>user\nHello there<|im_end|>\n<|im_start|>assistant\n");
    }

    #[test]
    fn supports_python_style_methods_and_raise_exception() {
        let t = "{{ bos_token }}{% for m in messages %}{{ m.content.strip() }}|{% if m.get('missing') %}x{% endif %}{% endfor %}";
        assert_eq!(render_chat_template(t, &conv(), "<bos>", "").unwrap(), "<bos>Be brief.|Hello there|");
        let bad = "{{ raise_exception('no system role allowed') }}";
        let err = render_chat_template(bad, &conv(), "", "").unwrap_err().to_string();
        assert!(err.contains("no system role allowed"), "{err}");
        assert!(render_chat_template("{% if %}", &conv(), "", "").unwrap_err().to_string().contains("parse"));
    }

    /// The real template shipped in the Gemma 4 GGUF (Apache-2.0), kept as a fixture so a minijinja
    /// upgrade or a template change cannot silently break prompt formatting.
    #[test]
    fn renders_the_gemma_4_template_exactly() {
        let t = include_str!("../../tests/fixtures/gemma-4-chat-template.jinja");
        let out = render_chat_template(t, &conv(), "<bos>", "<eos>").unwrap();
        assert_eq!(out, "<bos><|turn>system\nBe brief.<turn|>\n<|turn>user\nHello there<turn|>\n<|turn>model\n");
        // without a system message there is no system turn
        let no_sys = render_chat_template(t, &conv()[1..], "<bos>", "<eos>").unwrap();
        assert_eq!(no_sys, "<bos><|turn>user\nHello there<turn|>\n<|turn>model\n");
    }
}
