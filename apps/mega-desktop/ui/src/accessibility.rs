use gloo_timers::callback::Interval;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::components::{CHEVRON, DOT, Glyph, SEARCH, TARGET};
use crate::tauri::{
    AccessibilityAction, AccessibilityActionRequest, AccessibilityNode, AccessibilityStatus,
    PermissionCapability, PermissionState, accessibility_action, accessibility_start,
    accessibility_status, accessibility_stop, is_available, permission_open_settings,
    permission_request,
};

#[component]
pub fn Accessibility() -> impl IntoView {
    let (status, set_status) = signal(AccessibilityStatus::default());
    let (selected, set_selected) = signal(None::<AccessibilityNode>);
    let (busy, set_busy) = signal(false);
    let (message, set_message) = signal(None::<String>);
    let (value_draft, set_value_draft) = signal(String::new());

    let refresh = Callback::new(move |()| {
        if !is_available() || busy.get_untracked() {
            return;
        }
        spawn_local(async move {
            if let Ok(next) = accessibility_status().await {
                let generation = next.snapshot.as_ref().map(|snapshot| snapshot.generation);
                if selected
                    .get_untracked()
                    .and_then(|node| node.element)
                    .is_some_and(|element| Some(element.generation) != generation)
                {
                    set_selected.set(None);
                }
                set_status.set(next);
            }
        });
    });

    Effect::new(move |_| refresh.run(()));
    let _poller = StoredValue::new_local(Interval::new(800, move || refresh.run(())));

    let toggle_observation = move |_| {
        if busy.get_untracked() {
            return;
        }
        let should_stop = status.get_untracked().needs_stop();
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            let result = if should_stop {
                accessibility_stop().await
            } else {
                accessibility_start().await
            };
            match result {
                Ok(next) => {
                    set_status.set(next);
                    if should_stop {
                        set_selected.set(None);
                    }
                }
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let request_access = move |_| {
        if busy.get_untracked() {
            return;
        }
        let permission = status.get_untracked().permission;
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            let result = if permission.needs_settings() {
                permission_open_settings(PermissionCapability::Accessibility)
                    .await
                    .map(|_| permission)
            } else if permission == PermissionState::Granted {
                accessibility_status().await.map(|next| next.permission)
            } else {
                permission_request(PermissionCapability::Accessibility)
                    .await
                    .map(|statuses| statuses.accessibility)
            };
            match result {
                Ok(permission) => {
                    set_status.update(|current| current.permission = permission);
                    let copy = if permission == PermissionState::Granted {
                        "Accessibility access is ready."
                    } else if permission.needs_settings() {
                        "Open System Settings to change Accessibility access, then return to Stalky."
                    } else {
                        "Approve Stalky in the macOS permission sheet, then continue."
                    };
                    set_message.set(Some(copy.to_owned()));
                }
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    };

    let execute = Callback::new(move |action: AccessibilityAction| {
        if busy.get_untracked() {
            return;
        }
        let Some(node) = selected.get_untracked() else {
            set_message.set(Some("Select an interface element first.".to_owned()));
            return;
        };
        let Some(element) = node.element else {
            set_message.set(Some(
                "That interface element cannot be controlled.".to_owned(),
            ));
            return;
        };
        let value = (action == AccessibilityAction::SetValue).then(|| value_draft.get_untracked());
        set_busy.set(true);
        set_message.set(None);
        spawn_local(async move {
            let request = AccessibilityActionRequest {
                element,
                action,
                value,
            };
            match accessibility_action(request).await {
                Ok(result) if result.executed => {
                    set_message.set(Some(format!("{} completed.", action.label())));
                    refresh.run(());
                }
                Ok(_) => set_message.set(Some("The application declined that control.".to_owned())),
                Err(error) => set_message.set(Some(error)),
            }
            set_busy.set(false);
        });
    });

    view! {
        <div class="page accessibility-page">
            <header class="page-header accessibility-heading">
                <span>"Accessibility"</span>
                <h1>"Observe structure. Control with intent."</h1>
                <p>"A live, bounded view of the focused macOS interface. Stalky exposes only controls the selected element currently supports."</p>
                <div class="ax-heading-actions">
                    <button class="secondary-button" disabled=move || busy.get() on:click=request_access>
                        {move || if status.get().permission == PermissionState::Granted { "Check access" } else if status.get().permission.needs_settings() { "Open Settings" } else { "Request access" }}
                    </button>
                    <button class="primary-dock-button ax-primary" disabled=move || busy.get() on:click=toggle_observation>
                        {move || if busy.get() { "Working…" } else if status.get().needs_stop() { "Stop observation" } else { "Start observation" }}
                    </button>
                </div>
            </header>

            <div class="ax-status-strip">
                <div><span class="status-dot" class:good=move || status.get().is_running()></span><span>"Observer"</span><strong>{move || status.get().state.label()}</strong></div>
                <div><span>"Permission"</span><strong>{move || status.get().permission.label()}</strong></div>
                <div><span>"Events"</span><strong>{move || status.get().metrics.observed_events}</strong></div>
                <div><span>"Dropped"</span><strong>{move || status.get().metrics.dropped_events}</strong></div>
            </div>

            {move || message.get().map(|copy| view! { <div class="ax-message"><span>"Status"</span><p>{copy}</p></div> })}

            <div class="ax-layout">
                <section class="tree-card ax-tree-card">
                    <div class="tree-toolbar">
                        <span>{move || status.get().snapshot.as_ref().and_then(|snapshot| snapshot.application.as_ref()).and_then(|application| application.name.clone()).unwrap_or_else(|| "Focused hierarchy".to_owned())}</span>
                        <div><span class="status-dot" class:good=move || status.get().is_running()></span>{move || if status.get().is_running() { "Live" } else { "Stopped" }}</div>
                    </div>
                    <div class="ax-tree" role="tree" aria-label="Focused accessibility hierarchy">
                        {move || render_tree(status.get().snapshot.as_ref().and_then(|snapshot| snapshot.tree.as_ref()), selected, set_selected, set_value_draft)}
                    </div>
                </section>

                <section class="ax-control-card">
                    <div class="ax-control-header"><span>"Selected element"</span><strong>{move || selected.get().as_ref().and_then(node_title).unwrap_or_else(|| "Nothing selected".to_owned())}</strong></div>
                    {move || selected.get().map_or_else(
                        || view! { <div class="ax-empty"><Glyph paths=TARGET /><p>"Select a row in the live hierarchy to inspect its supported controls."</p></div> }.into_any(),
                        |node| render_controls(node, busy, value_draft, set_value_draft, execute).into_any(),
                    )}
                </section>
            </div>

            <div class="ax-lower-grid">
                <section class="settings-group ax-events">
                    <div class="ax-section-title"><h3>"Recent changes"</h3><span>"Newest first"</span></div>
                    {move || {
                        let events = status.get().recent_events;
                        if events.is_empty() {
                            view! { <div class="ax-empty compact"><p>"Events will appear while observation is running."</p></div> }.into_any()
                        } else {
                            events.into_iter().rev().take(8).map(|event| view! {
                                <div class="ax-event-row"><span>{event.sequence}</span><strong>{event.kind.label()}</strong><small>{event.element.map(|element| element.id).unwrap_or_else(|| "system".to_owned())}</small></div>
                            }).collect_view().into_any()
                        }
                    }}
                </section>
                <section class="settings-group ax-boundary">
                    <h3>"Control boundary"</h3>
                    <p>"Every action is initiated here, checked against the element’s current AX capabilities, and rejected when its generation is stale."</p>
                    <dl>
                        <div><dt>"Synthetic input"</dt><dd>"Disabled"</dd></div>
                        <div><dt>"Autonomous actions"</dt><dd>"Disabled"</dd></div>
                        <div><dt>"Snapshot retention"</dt><dd>"Memory only"</dd></div>
                    </dl>
                </section>
            </div>
        </div>
    }
}

fn render_tree(
    root: Option<&AccessibilityNode>,
    selected: ReadSignal<Option<AccessibilityNode>>,
    set_selected: WriteSignal<Option<AccessibilityNode>>,
    set_value_draft: WriteSignal<String>,
) -> AnyView {
    let Some(root) = root else {
        return view! { <div class="ax-empty"><Glyph paths=SEARCH /><p>"Start observation to inspect the focused application."</p></div> }.into_any();
    };
    let mut rows = Vec::new();
    flatten_tree(root, 0, &mut rows);
    rows.into_iter()
        .map(|(depth, node)| {
            let row_node = node.clone();
            let row_id = node.element.as_ref().map(|element| element.id.clone());
            let is_selected = move || {
                selected.get().as_ref().and_then(|current| current.element.as_ref()).map(|element| &element.id) == row_id.as_ref()
            };
            let role = node.role.clone().unwrap_or_else(|| "Element".to_owned()).trim_start_matches("AX").to_owned();
            let title = node_title(&node).unwrap_or_else(|| "Untitled".to_owned());
            view! {
                <button
                    class="tree-row ax-tree-row"
                    class:selected=is_selected
                    style=format!("--depth: {depth}")
                    role="treeitem"
                    on:click=move |_| {
                        set_value_draft.set(row_node.value.clone().filter(|value| value != "[redacted]").unwrap_or_default());
                        set_selected.set(Some(row_node.clone()));
                    }
                >
                    <span class="tree-chevron" aria-hidden="true">{if node.children.is_empty() { view! { <Glyph paths=DOT /> }.into_any() } else { view! { <Glyph paths=CHEVRON /> }.into_any() }}</span>
                    <span class="tree-role">{role}</span>
                    <strong>{title}</strong>
                    {node.focused.unwrap_or(false).then_some(view! { <span class="ax-focus-chip">"Focused"</span> })}
                </button>
            }
        })
        .collect_view()
        .into_any()
}

fn render_controls(
    node: AccessibilityNode,
    busy: ReadSignal<bool>,
    value_draft: ReadSignal<String>,
    set_value_draft: WriteSignal<String>,
    execute: Callback<AccessibilityAction>,
) -> impl IntoView {
    let actions = node.supported_actions.clone();
    let role = node
        .role
        .clone()
        .unwrap_or_else(|| "Unknown role".to_owned());
    let bounds = node.bounds.map(|bounds| {
        format!(
            "{:.0} × {:.0} at {:.0}, {:.0}",
            bounds.width, bounds.height, bounds.x, bounds.y
        )
    });
    view! {
        <div class="ax-control-body">
            <dl class="ax-properties">
                <div><dt>"Role"</dt><dd>{role}</dd></div>
                <div><dt>"Enabled"</dt><dd>{if node.enabled.unwrap_or(false) { "Yes" } else { "No" }}</dd></div>
                <div><dt>"Children"</dt><dd>{node.children_count}</dd></div>
                <div><dt>"Bounds"</dt><dd>{bounds.unwrap_or_else(|| "Unavailable".to_owned())}</dd></div>
            </dl>
            <div class="ax-actions">
                <span>"Supported controls"</span>
                <div>
                    {actions.into_iter().map(|action| view! {
                        <button disabled=move || busy.get() on:click=move |_| execute.run(action)>{action.label()}</button>
                    }).collect_view()}
                    {node.value_settable.then(|| view! {
                        <label class="ax-value-control">
                            <span>"Set value"</span>
                            <input maxlength="256" prop:value=move || value_draft.get() on:input=move |event| set_value_draft.set(event_target_value(&event)) />
                            <button disabled=move || busy.get() on:click=move |_| execute.run(AccessibilityAction::SetValue)>"Apply"</button>
                        </label>
                    })}
                </div>
                {(node.supported_actions.is_empty() && !node.value_settable).then(|| view! { <p class="ax-no-actions">"This element exposes no supported controls."</p> })}
            </div>
        </div>
    }
}

fn flatten_tree(
    node: &AccessibilityNode,
    depth: usize,
    rows: &mut Vec<(usize, AccessibilityNode)>,
) {
    rows.push((depth, node.clone()));
    for child in &node.children {
        flatten_tree(child, depth.saturating_add(1), rows);
    }
}

fn node_title(node: &AccessibilityNode) -> Option<String> {
    node.title
        .clone()
        .or_else(|| node.value.clone())
        .or_else(|| node.subrole.clone())
        .or_else(|| node.role.clone())
}
