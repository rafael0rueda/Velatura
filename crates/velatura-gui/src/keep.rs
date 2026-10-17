//! Replacing a view built afresh without losing the reader's place: the
//! views are rebuilt after every change, and a switch flipped at the
//! bottom of a page must leave the page, and the focus, where they were.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

/// How long a rebuilt page may take to reach the height it is scrolled
/// back to. Past that the page is left where it is.
const SETTLE: Duration = Duration::from_millis(500);

/// The first scrolled window in `widget`, itself included.
fn scroller(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    if let Some(scroll) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        return Some(scroll.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(scroll) = scroller(&c) {
            return Some(scroll);
        }
        child = c.next_sibling();
    }
    None
}

/// Where `focus` is inside `root`: the place among its siblings at each
/// level down.
fn path_to(root: &gtk::Widget, focus: &gtk::Widget) -> Option<Vec<usize>> {
    let mut path = Vec::new();
    let mut at = focus.clone();
    while &at != root {
        let parent = at.parent()?;
        let mut index = 0;
        let mut sibling = parent.first_child();
        while let Some(s) = sibling.filter(|s| s != &at) {
            index += 1;
            sibling = s.next_sibling();
        }
        path.push(index);
        at = parent;
    }
    path.reverse();
    Some(path)
}

fn follow(root: &gtk::Widget, path: &[usize]) -> Option<gtk::Widget> {
    let mut at = root.clone();
    for &index in path {
        let mut child = at.first_child();
        for _ in 0..index {
            child = child?.next_sibling();
        }
        at = child?;
    }
    Some(at)
}

/// Put `view` in `bin` in place of the view there, scrolled as far down as
/// that one was, and with the focus on the control in the same place if it
/// is the same kind of control.
pub fn replace(bin: &adw::Bin, view: &impl IsA<gtk::Widget>) {
    let view = view.as_ref();
    let old = bin.child();
    let scrolled = old
        .as_ref()
        .and_then(scroller)
        .map(|s| s.vadjustment().value())
        .filter(|v| *v > 0.0);
    let focus = old.as_ref().and_then(|old| {
        let focus = old.root()?.focus()?;
        let path = focus.is_ancestor(old).then(|| path_to(old, &focus))??;
        Some((path, focus.type_()))
    });
    bin.set_child(Some(view));

    // Elsewhere it stays on the window, as before: on the first control of
    // the page, the page would scroll to the top to show it.
    if let Some(same) =
        focus.and_then(|(path, kind)| follow(view, &path).filter(|w| w.type_() == kind))
    {
        same.grab_focus();
    }
    let (Some(at), Some(scroll)) = (scrolled, scroller(view)) else {
        return;
    };
    // The new page has no height until it is laid out: brought down as it
    // grows, until it is there or has settled.
    let adjustment = scroll.vadjustment();
    let handler = Rc::new(Cell::new(None));
    let id = adjustment.connect_upper_notify({
        let handler = handler.clone();
        move |adjustment| {
            adjustment.set_value(at.min(adjustment.upper() - adjustment.page_size()));
            if adjustment.value() >= at
                && let Some(id) = handler.take()
            {
                adjustment.disconnect(id);
            }
        }
    });
    handler.set(Some(id));
    glib::timeout_add_local_once(SETTLE, move || {
        if let Some(id) = handler.take() {
            adjustment.disconnect(id);
        }
    });
}
