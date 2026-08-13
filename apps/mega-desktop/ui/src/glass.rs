use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(
        catch,
        js_namespace = ["window", "__STALKY_GLASS__"],
        js_name = start
    )]
    async fn start_glass(canvas: web_sys::HtmlCanvasElement) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__STALKY_GLASS__"], js_name = stop)]
    fn stop_glass(controller: &JsValue);
}

#[component]
pub fn GlassSurface() -> impl IntoView {
    let canvas = NodeRef::<html::Canvas>::new();
    let started = StoredValue::new(false);
    let disposed = StoredValue::new(false);
    let controller = StoredValue::new_local(None::<JsValue>);

    Effect::new(move |_| {
        let Some(canvas) = canvas.get() else {
            return;
        };
        if started.get_value() {
            return;
        }
        started.set_value(true);
        spawn_local(async move {
            if let Ok(renderer) = start_glass(canvas).await {
                if disposed.get_value() {
                    stop_glass(&renderer);
                } else {
                    controller.set_value(Some(renderer));
                }
            }
        });
    });

    on_cleanup(move || {
        disposed.set_value(true);
        controller.update_value(|renderer| {
            if let Some(renderer) = renderer.take() {
                stop_glass(&renderer);
            }
        });
    });

    view! {
        <canvas
            node_ref=canvas
            class="glass-surface"
            aria-hidden="true"
        ></canvas>
    }
}
