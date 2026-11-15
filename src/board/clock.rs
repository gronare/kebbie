#[cfg(test)]
thread_local! {
    static PINNED: std::cell::Cell<Option<(u64, i64)>> = const { std::cell::Cell::new(None) };
    static PINNED_INSTANT: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn pin(at: u64, offset: i64) {
    PINNED.set(Some((at, offset)));
}

#[cfg(test)]
pub(super) fn pin_instant(at: std::time::Instant) {
    PINNED_INSTANT.set(Some(at));
}

pub(super) fn instant() -> std::time::Instant {
    #[cfg(test)]
    if let Some(at) = PINNED_INSTANT.get() {
        return at;
    }
    std::time::Instant::now()
}

pub(super) fn now() -> u64 {
    #[cfg(test)]
    if let Some((at, _)) = PINNED.get() {
        return at;
    }
    crate::messages::now()
}

pub(super) fn offset(at: i64) -> i64 {
    #[cfg(test)]
    if let Some((_, offset)) = PINNED.get() {
        return offset;
    }
    crate::recover::local_offset(at)
}
