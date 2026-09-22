//! Conservative archive budget plus a bounded, source-size-independent workspace.
const GIB: u64 = 1024 * 1024 * 1024;
const RESERVE: u64 = 8 * GIB;

#[derive(Clone, Copy)]
pub(crate) struct SourceSpace {
    archive: u64,
    readback: u64,
}
impl SourceSpace {
    pub(crate) fn new(payload: u64, entries: usize, creates_archive: bool) -> Self {
        // Include filesystem/archive entry overhead; assume no compression gain.
        let bytes = payload.saturating_add((entries as u64).saturating_mul(4096));
        let budget = bytes.saturating_add(bytes / 10);
        Self {
            archive: if creates_archive { budget } else { 0 },
            readback: crate::segmented::WORK_BYTES,
        }
    }
    pub(crate) fn required_before_creation(self) -> u64 {
        self.archive
            .saturating_add(self.readback)
            .saturating_add(2 * GIB)
    }
}

pub(crate) struct SpacePlan {
    pub order: Vec<usize>,
    pub required_bytes: u64,
    pub archive_bytes: u64,
}

pub(crate) fn plan(sources: &[SourceSpace]) -> SpacePlan {
    let mut order: Vec<_> = (0..sources.len()).collect();
    // Process large sources first; keep cached immutable manifests paired with
    // their original path indices. The verification workspace is bounded.
    order.sort_by_key(|&i| std::cmp::Reverse(sources[i].archive));
    let mut archives = 0u64;
    let mut peak = 0u64;
    for &i in &order {
        archives = archives.saturating_add(sources[i].archive);
        peak = peak.max(archives.saturating_add(sources[i].readback));
    }
    SpacePlan {
        order,
        required_bytes: peak.saturating_add(RESERVE),
        archive_bytes: archives,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reported_one_tb_drive_case_does_not_reserve_largest_copy_after_all_archives() {
        // Approximate the reported 580 GiB set: VM, photos, documents, others.
        let sizes = [82, 40, 298, 160];
        let sources: Vec<_> = sizes
            .iter()
            .map(|n| SourceSpace::new(n * GIB, 0, true))
            .collect();
        let p = plan(&sources);
        assert_eq!(p.order, [2, 3, 0, 1]);
        assert!(p.required_bytes < 700 * GIB);
        assert!(p.required_bytes < 931 * GIB);
        // Independently simulate accumulated archives plus the bounded workspace.
        let mut live_archives = 0;
        for i in p.order {
            live_archives += sources[i].archive;
            assert!(live_archives + sources[i].readback + RESERVE <= p.required_bytes);
        }
    }
    #[test]
    fn reused_archives_need_readback_space_but_no_second_archive() {
        let p = plan(&[
            SourceSpace::new(300 * GIB, 0, false),
            SourceSpace::new(10 * GIB, 0, true),
        ]);
        assert_eq!(p.archive_bytes, 11 * GIB);
        assert_eq!(
            p.required_bytes,
            11 * GIB + crate::segmented::WORK_BYTES + RESERVE
        );
    }
    #[test]
    fn single_large_source_needs_only_bounded_part_workspace() {
        let source = SourceSpace::new(500 * GIB, 0, true);
        let p = plan(&[source]);
        assert_eq!(
            p.required_bytes,
            550 * GIB + crate::segmented::WORK_BYTES + RESERVE
        );
        assert_eq!(
            source.required_before_creation(),
            550 * GIB + crate::segmented::WORK_BYTES + 2 * GIB
        );
        assert!(p.required_bytes < 931 * GIB);
    }
    #[test]
    fn entry_overhead_and_overflow_are_never_dropped() {
        let source = SourceSpace::new(0, 1_000_000, true);
        assert!(plan(&[source]).required_bytes > 4_000_000_000 + RESERVE);
        assert_eq!(
            plan(&[SourceSpace::new(u64::MAX, 1, true)]).required_bytes,
            u64::MAX
        );
        assert_eq!(plan(&[]).required_bytes, RESERVE);
    }
}
