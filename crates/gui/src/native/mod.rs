use crate::{Start, app::App, settings::Settings};
use dioxus::prelude::*;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};
mod components;
pub mod images;
pub mod platform;

#[derive(Clone)]
struct Initial(Rc<RefCell<Option<App>>>);
#[derive(Clone)]
struct NativeEvents(async_channel::Receiver<platform::NativeEvent>);
#[derive(Clone)]
struct Saved(Arc<Mutex<Snapshot>>);
struct Snapshot {
    settings: Settings,
    run_cancel: reconst_prep_core::cancel::CancelToken,
    preview_cancel: reconst_prep_core::cancel::CancelToken,
}
impl Snapshot {
    fn from_app(app: &App) -> Self {
        Self {
            settings: app.to_settings(),
            run_cancel: app.cancel.clone(),
            preview_cancel: app.preview_cancel(),
        }
    }
}

pub fn run(start: Start) -> anyhow::Result<()> {
    let settings = Settings::load();
    let size = settings.window_size.unwrap_or([1360.0, 900.0]);
    let app = App::from_settings(settings).with(start);
    run_app(app, size)
}

pub(crate) fn run_app(app: App, size: [f32; 2]) -> anyhow::Result<()> {
    let saved = Arc::new(Mutex::new(Snapshot::from_app(&app)));
    let (tx, rx) = async_channel::unbounded();
    let mut vdom = VirtualDom::new(root);
    vdom.insert_any_root_context(Box::new(Initial(Rc::new(RefCell::new(Some(app))))));
    vdom.insert_any_root_context(Box::new(NativeEvents(rx)));
    vdom.insert_any_root_context(Box::new(Saved(saved.clone())));
    platform::run(vdom, size, tx, move |size| {
        let mut snapshot = saved.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(size) = size {
            snapshot.settings.window_size = Some(size);
        }
        snapshot.settings.save();
        snapshot.run_cancel.cancel();
        snapshot.preview_cancel.cancel();
    })
}

/// All mutations run on the UI thread; workers only send messages.
pub fn change(mut app: Signal<App>, action: impl FnOnce(&mut App)) {
    let mut app = app.write();
    action(&mut app);
    app.reconcile();
    if let Some(saved) = try_consume_context::<Saved>() {
        *saved.0.lock().unwrap_or_else(|p| p.into_inner()) = Snapshot::from_app(&app);
    }
}

fn root() -> Element {
    platform::use_native_events();
    let initial = use_context::<Initial>();
    let mut app = use_signal(|| initial.0.borrow_mut().take().expect("root mounted once"));
    let saved = use_context::<Saved>();
    let native = use_context::<NativeEvents>();
    let mut hovered = use_signal(|| false);
    let commands = use_context::<platform::NativeCommands>();
    let title = use_memo(move || app.read().window_title());
    use_effect(move || commands.set_title(title()));
    #[cfg(test)]
    crate::screenshots::observe(app);
    use_context_provider(|| app);
    use_future(move || {
        let rx = app.read().channel.rx.clone();
        async move {
            change(app, |_| {});
            while let Ok(msg) = rx.recv().await {
                change(app, |app| app.on_message(msg));
            }
        }
    });
    use_future(move || {
        let rx = native.0.clone();
        async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    platform::NativeEvent::Dropped(path) => {
                        hovered.set(false);
                        change(app, |app| app.take_dropped(vec![path]));
                    }
                    platform::NativeEvent::Resize(size) => {
                        change(app, |app| app.cfg.window_size = Some(size))
                    }
                    platform::NativeEvent::Hover(value) => hovered.set(value),
                    platform::NativeEvent::Close => {
                        app.write().shutdown();
                        break;
                    }
                }
            }
        }
    });
    use_effect(move || {
        *saved.0.lock().unwrap_or_else(|p| p.into_inner()) = Snapshot::from_app(&app.read());
    });
    rsx! {
        style { {include_str!("theme.css")} }
        style { {include_str!("style.css")} }
        style { {include_str!("components/button.css")} }
        style { {include_str!("components/inspector.css")} }
        style { {include_str!("components/preview.css")} }
        style { {include_str!("components/status.css")} }
        components::Workspace {}
        if hovered() { div { class: "drop-overlay", "Drop videos, folders, lens profiles or models" } }
    }
}
