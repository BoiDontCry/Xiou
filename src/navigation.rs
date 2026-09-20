//! Small mail-domain adapters for the official virtualized navigation widgets.
use gpui::{px, size, Pixels, Size};
use std::{collections::{HashMap, VecDeque}, rc::Rc};

/// Keep layout metadata stable across redraws; only count/rem-size changes allocate.
#[derive(Default)]
pub(crate) struct VirtualRows {
    height: Pixels,
    sizes: Rc<Vec<Size<Pixels>>>,
}

impl VirtualRows {
    pub fn sizes(&mut self, count: usize, height: Pixels) -> Rc<Vec<Size<Pixels>>> {
        if self.sizes.len() != count || self.height != height {
            self.height = height;
            self.sizes = Rc::new(vec![size(px(0.), height); count]);
        }
        self.sizes.clone()
    }
}

/// Bounded LRU keeps a long scroll from retaining every queried message page.
pub(crate) fn touch_page<T>(pages: &mut HashMap<usize, T>, order: &mut VecDeque<usize>, page: usize, capacity: usize) {
    order.retain(|key| pages.contains_key(key) && *key != page);
    if pages.contains_key(&page) { order.push_back(page); }
    while pages.len() > capacity.max(1) {
        if let Some(oldest) = order.pop_front() { pages.remove(&oldest); }
        else { break; }
    }
}

pub(crate) fn navigation_step(current: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 { return None; }
    match current.filter(|index| *index < count) {
        Some(index) => crate::ui::step_index(Some(index),count,delta),
        None => Some(if delta < 0 { count - 1 } else { 0 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtual_row_metrics_are_reused_until_count_or_height_changes() {
        let mut rows=VirtualRows::default();
        let initial=rows.sizes(10_000,px(76.));
        assert!(Rc::ptr_eq(&initial,&rows.sizes(10_000,px(76.))));
        assert!(!Rc::ptr_eq(&initial,&rows.sizes(10_000,px(80.))));
        assert_eq!(rows.sizes(0,px(80.)).len(),0);
    }
    #[test]
    fn page_cache_evicts_least_recently_used_without_losing_current_page() {
        let mut pages=HashMap::new(); let mut order=VecDeque::new();
        for page in 0..10 { pages.insert(page,page); touch_page(&mut pages,&mut order,page,3); }
        assert_eq!(pages.len(),3);
        touch_page(&mut pages,&mut order,7,3);
        pages.insert(10,10); touch_page(&mut pages,&mut order,10,3);
        assert!(pages.contains_key(&7) && pages.contains_key(&10));
        assert!(!pages.contains_key(&8));
        pages.clear(); pages.insert(1,1); touch_page(&mut pages,&mut order,1,3);
        assert_eq!(order,VecDeque::from([1]));
    }
    #[test]
    fn initial_navigation_does_not_skip_first_or_last_row() {
        assert_eq!(navigation_step(None,51,1),Some(0));
        assert_eq!(navigation_step(None,51,-1),Some(50));
        assert_eq!(navigation_step(Some(50),51,1),Some(0));
        assert_eq!(navigation_step(Some(0),51,-1),Some(50));
        assert_eq!(navigation_step(Some(100),3,1),Some(0));
        assert_eq!(navigation_step(None,0,1),None);
    }
}
