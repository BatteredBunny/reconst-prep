//! Window lifecycle only; Dioxus Native owns input dispatch, resources and rendering.
use async_channel::Sender;
#[cfg(test)]
use blitz_shell::BlitzShellEvent;
use blitz_shell::{BlitzShellProxy, WindowConfig};
use dioxus::prelude::*;
use dioxus_native::winit;
use dioxus_native::{
    DioxusDocument, DioxusNativeApplication, DioxusNativeWindowRenderer, DocumentConfig,
};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};
use winit::{
    data_transfer::TypeHint,
    event::WindowEvent,
    event_loop::{DndAction, EventLoop},
    window::{Window, WindowAttributes},
};

#[derive(Debug)]
pub enum NativeEvent {
    Dropped(PathBuf),
    Resize([f32; 2]),
    Close,
    Hover(bool),
}

type SaveOnClose = Rc<RefCell<Box<dyn FnMut(Option<[f32; 2]>)>>>;

#[derive(Clone)]
struct Lifecycle {
    events: Sender<NativeEvent>,
    #[cfg(test)]
    proxy: BlitzShellProxy,
    save: SaveOnClose,
    closed: Rc<Cell<bool>>,
}
impl Lifecycle {
    fn close(&self, size: Option<[f32; 2]>) {
        if !self.closed.replace(true) {
            (self.save.borrow_mut())(size);
            let _ = self.events.try_send(NativeEvent::Close);
        }
    }
    fn emit(&self, event: NativeEvent) {
        let _ = self.events.try_send(event);
    }
}
#[derive(Clone)]
pub struct NativeCommands {
    window: Arc<dyn Window>,
    #[cfg(test)]
    lifecycle: Lifecycle,
}
impl NativeCommands {
    #[cfg(test)]
    pub fn close(&self) {
        let size = self
            .window
            .surface_size()
            .to_logical::<f32>(self.window.scale_factor());
        self.lifecycle.close(Some([size.width, size.height]));
        self.lifecycle
            .proxy
            .send_event(BlitzShellEvent::CloseWindow {
                window_id: self.window.id(),
            });
    }
    pub fn set_title(&self, title: String) {
        self.window.set_title(&title);
    }
    pub fn open_folder(&self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Ok(url) = blitz_traits::net::Url::from_directory_path(path)
            && let Err(error) = webbrowser::open(url.as_str())
        {
            log::warn!("Cannot open folder: {error}");
        }
    }
}

/// Installed once by the application root after the native window context exists.
pub fn use_native_events() {
    let window = dioxus_native::use_window();
    let lifecycle = use_context::<Lifecycle>();
    use_context_provider(|| NativeCommands {
        window: window.clone(),
        #[cfg(test)]
        lifecycle: lifecycle.clone(),
    });
    let mut pending = None;
    let mut dropped = false;
    let mut paths = Vec::new();
    dioxus_native::use_window_event(move |event, target| match event {
        WindowEvent::SurfaceResized(size) => {
            let size = size.to_logical::<f32>(window.scale_factor());
            if size.width > 0. && size.height > 0. {
                lifecycle.emit(NativeEvent::Resize([size.width, size.height]));
            }
        }
        WindowEvent::CloseRequested => {
            let size = window
                .surface_size()
                .to_logical::<f32>(window.scale_factor());
            lifecycle.close(Some([size.width, size.height]));
        }
        WindowEvent::DragEntered { id, .. } => {
            paths.clear();
            dropped = false;
            pending = None;
            if target
                .data_transfer(*id)
                .is_ok_and(|data| data.has_type(&TypeHint::UriList))
            {
                let _ = target.set_valid_dnd_actions(*id, &[DndAction::Copy]);
                pending = target.fetch_data_transfer(*id, &TypeHint::UriList).ok();
                lifecycle.emit(NativeEvent::Hover(true));
            }
        }
        WindowEvent::DataTransferReceived { serial, value, .. } if Some(*serial) == pending => {
            pending = None;
            if let Ok(uris) = value.try_as_uris() {
                paths = uris
                    .iter()
                    .filter_map(|uri| blitz_traits::net::Url::parse(uri).ok()?.to_file_path().ok())
                    .collect();
            }
            if dropped {
                for path in paths.drain(..) {
                    lifecycle.emit(NativeEvent::Dropped(path));
                }
            }
        }
        WindowEvent::DragDropped { .. } => {
            dropped = true;
            lifecycle.emit(NativeEvent::Hover(false));
            for path in paths.drain(..) {
                lifecycle.emit(NativeEvent::Dropped(path));
            }
        }
        WindowEvent::DragLeft { .. } => {
            lifecycle.emit(NativeEvent::Hover(false));
            if !dropped {
                paths.clear();
                pending = None;
            }
        }
        _ => {}
    });
}

struct ExternalNavigation;
impl blitz_traits::navigation::NavigationProvider for ExternalNavigation {
    fn navigate_to(&self, options: blitz_traits::navigation::NavigationOptions) {
        if options.method == blitz_traits::net::Method::GET
            && matches!(options.url.scheme(), "http" | "https" | "mailto")
            && let Err(error) = webbrowser::open(options.url.as_str())
        {
            log::warn!("Cannot open link: {error}");
        }
    }
}

pub fn run(
    mut vdom: VirtualDom,
    size: [f32; 2],
    events: Sender<NativeEvent>,
    on_close: impl FnMut(Option<[f32; 2]>) + 'static,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let _guard = runtime.enter();
    let mut builder = EventLoop::builder();
    #[cfg(all(test, target_os = "linux"))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11().with_any_thread(true);
    }
    let event_loop = builder.build()?;
    let (proxy, queue) = BlitzShellProxy::new(event_loop.create_proxy());
    let lifecycle = Lifecycle {
        events,
        #[cfg(test)]
        proxy: proxy.clone(),
        save: Rc::new(RefCell::new(Box::new(on_close))),
        closed: Rc::new(Cell::new(false)),
    };
    vdom.insert_any_root_context(Box::new(lifecycle.clone()));
    let mut fonts = dioxus_native::FontContext::default();
    for face in [
        include_bytes!("../../assets/fonts/NordwandMono-Regular.ttf").as_slice(),
        include_bytes!("../../assets/fonts/NordwandMono-Medium.ttf").as_slice(),
    ] {
        fonts
            .collection
            .register_fonts(peniko::Blob::from(face.to_vec()), None);
    }
    let doc = DioxusDocument::new(
        vdom,
        DocumentConfig {
            font_ctx: Some(fonts),
            navigation_provider: Some(Arc::new(ExternalNavigation)),
            ..Default::default()
        },
    );
    let icon =
        image::load_from_memory(include_bytes!("../../../../assets/icon-256.png"))?.into_rgba8();
    let attributes = WindowAttributes::default()
        .with_title("reconst-prep")
        .with_surface_size(winit::dpi::LogicalSize::new(size[0], size[1]))
        .with_min_surface_size(winit::dpi::LogicalSize::new(900., 600.))
        .with_window_icon(Some(
            winit::icon::RgbaIcon::new(icon.as_raw().clone(), icon.width(), icon.height())?.into(),
        ));
    let config =
        WindowConfig::with_attributes(Box::new(doc), DioxusNativeWindowRenderer::new(), attributes);
    let app = DioxusNativeApplication::new(proxy, queue, config);
    let result = event_loop.run_app(app);
    lifecycle.close(None);
    result?;
    Ok(())
}
