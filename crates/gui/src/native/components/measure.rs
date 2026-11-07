//! Reading laid-out geometry. Blitz lays a change out on the next frame and has no resize
//! observer, so components that size themselves from layout read it a frame later.
//! TODO: replace with native implementation once fixed by blitz (onresize / ResizeObserver).
use super::*;

/// Where a component keeps the element it measures.
pub(super) type MountedSlot = Signal<Option<std::rc::Rc<MountedData>>>;

/// The element's rect once the current change has been laid out, or `None` while it has no size.
pub(super) async fn settled_rect(
    mounted: &MountedData,
) -> Option<dioxus::html::geometry::PixelsRect> {
    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
    mounted
        .get_client_rect()
        .await
        .ok()
        .filter(|rect| rect.size.width > 0. && rect.size.height > 0.)
}
