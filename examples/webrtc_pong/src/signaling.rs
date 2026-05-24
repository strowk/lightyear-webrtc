use lightyear_webrtc::signaling::{BoxFuture, SignalingClient, SignalingError};
use wasm_bindgen::JsCast;

extern crate alloc;
use alloc::format;
use alloc::string::String;
use base64::Engine;

pub(crate) struct ManualSignaling;

unsafe impl Send for ManualSignaling {}
unsafe impl Sync for ManualSignaling {}

impl ManualSignaling {
    /// Show a textarea with text for the user to copy, plus a "Done" button.
    fn show_text(label: &str, text: &str) {
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = Self::get_or_create_signaling_div(&doc);
        container.set_inner_html(&format!(
            r#"<p style="margin:0 0 4px"><b>{label}</b></p>
            <textarea id="sdp-output" rows="6" style="width:100%;font-size:11px;font-family:monospace" readonly></textarea>
            <br><button id="sdp-copy" style="margin:4px 2px">Copy</button>
            <button id="sdp-done" style="margin:4px 2px">Done</button>"#
        ));
        // Set value via property (not innerHTML) to preserve exact content
        if let Some(ta) = doc.get_element_by_id("sdp-output") {
            let ta: web_sys::HtmlTextAreaElement = ta.unchecked_into();
            ta.set_value(text);
            ta.select();
        }
        if let Some(btn) = doc.get_element_by_id("sdp-copy") {
            let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                if let Some(w) = web_sys::window() {
                    if let Some(doc) = w.document() {
                        if let Some(ta) = doc.get_element_by_id("sdp-output") {
                            let ta: web_sys::HtmlTextAreaElement = ta.unchecked_into();
                            ta.select();
                            let _ = w.navigator().clipboard().write_text(&ta.value());
                        }
                    }
                }
            });
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    }

    /// Show a textarea for the user to paste into, plus a "Submit" button.
    /// Resolves with the pasted text when Submit is clicked.
    async fn prompt_text(label: &str) -> Result<String, SignalingError> {
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = Self::get_or_create_signaling_div(&doc);
        container.set_inner_html(&format!(
            r#"<p style="margin:0 0 4px"><b>{label}</b></p>
            <textarea id="sdp-input" rows="4" style="width:100%;font-size:11px;font-family:monospace" placeholder="Paste here..."></textarea>
            <br><button id="sdp-submit" style="margin:4px 0">Submit</button>"#
        ));

        let (tx, rx) = futures_channel::oneshot::channel::<String>();
        let tx = std::sync::Mutex::new(Some(tx));
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                if let Some(ta) = doc.get_element_by_id("sdp-input") {
                    let ta: web_sys::HtmlTextAreaElement = ta.unchecked_into();
                    let value = ta.value();
                    if let Some(tx) = tx.lock().unwrap().take() {
                        let _ = tx.send(value);
                    }
                }
            }
        });
        if let Some(btn) = doc.get_element_by_id("sdp-submit") {
            let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
        }
        cb.forget();

        rx.await.map_err(|_| SignalingError::Failed("Submit cancelled".into()))
    }

    /// Resolves when the user clicks the "Done" button.
    async fn wait_for_done() {
        let (tx, rx) = futures_channel::oneshot::channel::<()>();
        let tx = std::sync::Mutex::new(Some(tx));
        let cb = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        });
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(btn) = doc.get_element_by_id("sdp-done") {
                let _ = btn.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
            }
        }
        cb.forget();
        let _ = rx.await;
    }

    fn clear_signaling_ui() {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(el) = doc.get_element_by_id("signaling-ui") {
                el.set_inner_html("<p style='color:#4fc3f7'><b>Connected!</b></p>");
            }
        }
    }

    fn get_or_create_signaling_div(doc: &web_sys::Document) -> web_sys::Element {
        if let Some(el) = doc.get_element_by_id("signaling-ui") {
            return el;
        }
        let div = doc.create_element("div").unwrap();
        div.set_id("signaling-ui");
        let _ = div.set_attribute("style",
            "position:fixed;top:10px;left:10px;z-index:9999;background:#222;color:#eee;\
             padding:12px;border-radius:8px;max-width:500px;font-family:monospace;font-size:13px");
        doc.body().unwrap().append_child(&div).unwrap();
        div
    }
}

fn encode_sdp(sdp: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(sdp.as_bytes())
}

fn decode_sdp(encoded: &str) -> Result<String, SignalingError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim().as_bytes())
        .map_err(|e| SignalingError::Failed(format!("base64 decode failed: {e}")))?;
    String::from_utf8(bytes)
        .map_err(|e| SignalingError::Failed(format!("invalid UTF-8: {e}")))
}

impl SignalingClient for ManualSignaling {
    fn publish_offer(&mut self, offer: String) -> BoxFuture<'_, Result<String, SignalingError>> {
        Box::pin(async move {
            let encoded = encode_sdp(&offer);
            Self::show_text("OFFER - copy this, open client tab, paste it there:", &encoded);
            Self::wait_for_done().await;

            let answer_encoded = Self::prompt_text("Paste the ANSWER from the client tab:").await?;
            let answer = decode_sdp(&answer_encoded)?;

            Self::clear_signaling_ui();
            Ok(answer)
        })
    }

    fn retrieve_offer(&mut self) -> BoxFuture<'_, Result<String, SignalingError>> {
        Box::pin(async move {
            let offer_encoded = Self::prompt_text("Paste the OFFER from the host tab:").await?;
            decode_sdp(&offer_encoded)
        })
    }

    fn submit_answer(&mut self, answer: String) -> BoxFuture<'_, Result<(), SignalingError>> {
        Box::pin(async move {
            let encoded = encode_sdp(&answer);
            Self::show_text("ANSWER - copy this, go to the host tab, paste it there:", &encoded);
            Self::wait_for_done().await;
            Self::clear_signaling_ui();
            Ok(())
        })
    }
}
