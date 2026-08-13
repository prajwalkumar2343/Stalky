use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
    let started = Rc::new(Cell::new(false));
    let disposed = Rc::new(Cell::new(false));
    let controller = Rc::new(RefCell::new(None::<JsValue>));

    Effect::new({
        let started = Rc::clone(&started);
        let disposed = Rc::clone(&disposed);
        let controller = Rc::clone(&controller);
        move |_| {
            let Some(canvas) = canvas.get() else {
                return;
            };
            if started.replace(true) {
                return;
            }
            let disposed = Rc::clone(&disposed);
            let controller = Rc::clone(&controller);
            spawn_local(async move {
                if let Ok(renderer) = start_glass(canvas).await {
                    if disposed.get() {
                        stop_glass(&renderer);
                    } else {
                        controller.replace(Some(renderer));
                    }
                }
            });
        }
    });

    on_cleanup(move || {
        disposed.set(true);
        if let Some(renderer) = controller.borrow_mut().take() {
            stop_glass(&renderer);
        }
    });

    view! {
        <canvas
            node_ref=canvas
            class="glass-surface"
            aria-hidden="true"
        ></canvas>
    }
}
