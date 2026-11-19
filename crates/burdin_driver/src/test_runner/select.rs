//! Which tests run: name filters, `--exact`, `--skip`, `--failed`, and `--shard` (`spec/test-runner.md`, "Discovery
//! and selection"), and the record of failed tests `--failed` reads.

use super::{Plan, Planned, TestCase};
use crate::Args;
use std::path::Path;
use std::time::Duration;

/// Whether `pattern` selects the full path `path`: contained in it, or equal to it with `--exact`. The comparison is
/// case-sensitive.
pub(super) fn pattern_matches(pattern: &str, path: &str, exact: bool) -> bool {
    if exact { pattern == path } else { path.contains(pattern) }
}

/// A test is selected by name when no filter was given or any filter matches, and no `--skip` pattern matches.
pub(crate) fn name_selected(args: &Args, path: &str) -> bool {
    let wanted = args.filters.is_empty() || args.filters.iter().any(|filter| pattern_matches(filter, path, args.exact));
    wanted && !args.skip.iter().any(|pattern| pattern_matches(pattern, path, args.exact))
}

/// Whether the 0-based position `position` among the selected tests belongs to shard `index` of `count` (counted from
/// 1). The slices are taken round-robin, so every selected test is in exactly one shard.
pub(super) fn in_shard(position: usize, (index, count): (usize, usize)) -> bool {
    position % count + 1 == index
}

/// Applies the selection rules to the discovered tests, in this order: the name filters and `--skip`, `--failed`,
/// the ignore mode, and `--shard`. A test the name rules, `--failed`, or `--shard` leave out is counted as filtered
/// out; `--ignored` drops the tests that are not ignored without counting them, as it always has.
pub(super) fn plan_tests(
    args: &Args,
    cases: Vec<TestCase>,
    failed: Option<&FailedRecord>,
    default_timeout: Option<Duration>,
) -> Plan {
    let mut filtered_out = 0usize;
    let mut kept = Vec::new();
    for case in cases {
        let by_name = name_selected(args, &case.path);
        let by_record = failed.is_none_or(|record| record.contains(&case.package, &case.local));
        if !by_name || !by_record {
            filtered_out += 1;
        } else if !args.ignored_only || case.attrs.ignore {
            // `--ignored` runs only the ignored tests; the others are not "filtered out" of that run.
            kept.push(case);
        }
    }
    let mut plan = Plan { tests: Vec::new(), ignored: 0, filtered_out };
    for (position, case) in kept.into_iter().enumerate() {
        if args.shard.is_some_and(|shard| !in_shard(position, shard)) {
            plan.filtered_out += 1;
            continue;
        }
        let ignored = case.attrs.ignore && !args.ignored_only && !args.include_ignored;
        if ignored {
            plan.ignored += 1;
        }
        let timeout = match case.attrs.timeout_ms {
            Some(0) => None,
            Some(ms) => Some(Duration::from_millis(ms)),
            None => default_timeout,
        };
        plan.tests.push(Planned { case, ignored, timeout });
    }
    plan
}

/// The tests that failed in the previous run, as `(package, local path)` pairs: the identity of a test that does not
/// depend on which packages a run selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FailedRecord {
    entries: Vec<(String, String)>,
}

const RECORD_HEADER: &str = "burdin-test-failed 1";

impl FailedRecord {
    pub(super) fn contains(&self, package: &str, local: &str) -> bool {
        self.entries.iter().any(|(p, l)| p == package && l == local)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The record after a run. A test that ran and passed leaves the record, a test that failed enters it, and a test
    /// the run did not touch keeps its status. An entry whose test no longer exists in a package the run compiled is
    /// dropped; the entries of packages the run did not select stay.
    pub(super) fn updated(
        previous: Option<&FailedRecord>,
        selected_packages: &[String],
        discovered: &[(String, String)],
        passed: &[(String, String)],
        failed: &[(String, String)],
    ) -> FailedRecord {
        let mut entries: Vec<(String, String)> = Vec::new();
        for entry in previous.map(|record| record.entries.as_slice()).unwrap_or_default() {
            let selected = selected_packages.contains(&entry.0);
            if !selected || (discovered.contains(entry) && !passed.contains(entry)) {
                entries.push(entry.clone());
            }
        }
        entries.extend(failed.iter().cloned());
        entries.sort();
        entries.dedup();
        FailedRecord { entries }
    }

    /// Reads a record; `None` when the file is missing, unreadable, or not a record of this version.
    pub(super) fn load(path: &Path) -> Option<FailedRecord> {
        let text = std::fs::read_to_string(path).ok()?;
        FailedRecord::parse(&text)
    }

    pub(super) fn parse(text: &str) -> Option<FailedRecord> {
        let mut lines = text.lines();
        if lines.next()? != RECORD_HEADER {
            return None;
        }
        let mut entries = Vec::new();
        for line in lines.filter(|line| !line.is_empty()) {
            let (package, local) = line.split_once('\t')?;
            entries.push((unescape(package), unescape(local)));
        }
        Some(FailedRecord { entries })
    }

    pub(super) fn render(&self) -> String {
        let mut text = format!("{RECORD_HEADER}\n");
        for (package, local) in &self.entries {
            text.push_str(&format!("{}\t{}\n", escape(package), escape(local)));
        }
        text
    }

    /// Writes the record next to the package's other build output. A green run with no record on disk writes
    /// nothing, so a suite that never fails leaves no files behind. The file is replaced in one step.
    pub(super) fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.entries.is_empty() && !path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&temporary, self.render())?;
        std::fs::rename(&temporary, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
    }
}

/// Test names may contain any character a string literal can, so a record line escapes the separators.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch => out.push(ch),
        }
    }
    out
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_runner::{Attrs, Run};
    use std::path::PathBuf;

    fn case(package: &str, module: &str, name: &str, ignore: bool) -> TestCase {
        let local = if module.is_empty() { name.to_string() } else { format!("{module}.{name}") };
        TestCase {
            package: package.to_string(),
            module: module.to_string(),
            name: name.to_string(),
            local: local.clone(),
            display: name.to_string(),
            path: local,
            file: "src/main.bd".to_string(),
            line: 1,
            run: Run::Block { exe: PathBuf::from("bin"), index: 0, comptime: None },
            attrs: Attrs { ignore, ..Attrs::default() },
        }
    }

    fn paths(plan: &Plan) -> Vec<&str> {
        plan.tests.iter().map(|planned| planned.case.path.as_str()).collect()
    }

    fn cases() -> Vec<TestCase> {
        vec![
            case("app", "", "alpha", false),
            case("app", "geometry", "area", false),
            case("app", "geometry.shapes", "area of circle", false),
            case("app", "", "beta", true),
        ]
    }

    #[test]
    fn substring_filters_match_the_full_path() {
        let args = Args { filters: vec!["geometry".to_string()], ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!(paths(&plan), ["geometry.area", "geometry.shapes.area of circle"]);
        assert_eq!(plan.filtered_out, 2);

        let args = Args { filters: vec!["alpha".to_string(), "circle".to_string()], ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!(paths(&plan), ["alpha", "geometry.shapes.area of circle"]);
        assert_eq!(plan.filtered_out, 2);
    }

    #[test]
    fn exact_requires_the_whole_path() {
        let args = Args { filters: vec!["area".to_string()], exact: true, ..Args::default() };
        assert!(plan_tests(&args, cases(), None, None).tests.is_empty());

        let args = Args { filters: vec!["geometry.area".to_string()], exact: true, ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!(paths(&plan), ["geometry.area"]);
        assert_eq!(plan.filtered_out, 3);
    }

    #[test]
    fn skip_removes_tests_and_follows_exact() {
        let args = Args { skip: vec!["geometry".to_string()], ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!(paths(&plan), ["alpha", "beta"]);
        assert_eq!(plan.filtered_out, 2);

        let args = Args { skip: vec!["geometry".to_string()], exact: true, ..Args::default() };
        assert_eq!(plan_tests(&args, cases(), None, None).tests.len(), 4);

        let args = Args { filters: vec!["area".to_string()], skip: vec!["circle".to_string()], ..Args::default() };
        assert_eq!(paths(&plan_tests(&args, cases(), None, None)), ["geometry.area"]);
    }

    #[test]
    fn the_ignore_modes_still_apply_after_the_name_filters() {
        let plan = plan_tests(&Args::default(), cases(), None, None);
        assert_eq!((plan.tests.len(), plan.ignored, plan.filtered_out), (4, 1, 0));

        let args = Args { ignored_only: true, ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!((paths(&plan), plan.ignored, plan.filtered_out), (vec!["beta"], 0, 0));

        let args = Args { include_ignored: true, ..Args::default() };
        let plan = plan_tests(&args, cases(), None, None);
        assert_eq!((plan.tests.len(), plan.ignored), (4, 0));
    }

    #[test]
    fn shards_partition_the_selected_tests() {
        let mut seen = Vec::new();
        for index in 1..=3 {
            let args = Args { shard: Some((index, 3)), ..Args::default() };
            let plan = plan_tests(&args, cases(), None, None);
            assert_eq!(plan.tests.len() + plan.filtered_out, 4);
            seen.extend(paths(&plan).into_iter().map(str::to_string));
        }
        seen.sort();
        let mut all: Vec<String> = cases().into_iter().map(|case| case.path).collect();
        all.sort();
        assert_eq!(seen, all);

        // The first shard of two takes the first and third test in declaration order.
        let args = Args { shard: Some((1, 2)), ..Args::default() };
        assert_eq!(paths(&plan_tests(&args, cases(), None, None)), ["alpha", "geometry.shapes.area of circle"]);
    }

    #[test]
    fn failed_keeps_only_recorded_tests() {
        let record = FailedRecord { entries: vec![("app".to_string(), "geometry.area".to_string())] };
        let args = Args { failed: true, ..Args::default() };
        let plan = plan_tests(&args, cases(), Some(&record), None);
        assert_eq!(paths(&plan), ["geometry.area"]);
        assert_eq!(plan.filtered_out, 3);
    }

    #[test]
    fn a_timeout_attribute_overrides_the_default() {
        let mut slow = case("app", "", "slow", false);
        slow.attrs.timeout_ms = Some(0);
        let mut quick = case("app", "", "quick", false);
        quick.attrs.timeout_ms = Some(50);
        let other = case("app", "", "other", false);
        let default = Some(Duration::from_millis(1000));
        let plan = plan_tests(&Args::default(), vec![slow, quick, other], None, default);
        let timeouts: Vec<_> = plan.tests.iter().map(|planned| planned.timeout).collect();
        assert_eq!(timeouts, [None, Some(Duration::from_millis(50)), default]);
    }

    fn id(package: &str, local: &str) -> (String, String) {
        (package.to_string(), local.to_string())
    }

    #[test]
    fn the_record_follows_the_run() {
        let previous = FailedRecord { entries: vec![id("app", "old fail"), id("app", "gone"), id("lib", "kept")] };
        let discovered = [id("app", "old fail"), id("app", "new fail"), id("app", "fixed")];
        let updated = FailedRecord::updated(
            Some(&previous),
            &["app".to_string()],
            &discovered,
            &[id("app", "fixed")],
            &[id("app", "new fail")],
        );
        // `gone` no longer exists in `app`, `lib` was not selected, and `old fail` was not rerun.
        assert_eq!(updated.entries, [id("app", "new fail"), id("app", "old fail"), id("lib", "kept")]);

        let healed =
            FailedRecord::updated(Some(&updated), &["app".to_string(), "lib".to_string()], &discovered, &[], &[]);
        // `kept` is not discovered in `lib` any more; the two `app` failures were not rerun.
        assert_eq!(healed.entries, [id("app", "new fail"), id("app", "old fail")]);
    }

    #[test]
    fn the_record_round_trips_awkward_names() {
        let record = FailedRecord { entries: vec![id("app", "tab\there"), id("app", "back\\slash\nnewline")] };
        assert_eq!(FailedRecord::parse(&record.render()), Some(record));
        assert_eq!(FailedRecord::parse("something else\n"), None);
        assert_eq!(FailedRecord::parse(""), None);
    }

    #[test]
    fn saving_a_green_run_writes_nothing_new() {
        let dir = crate::TempDir::new().unwrap();
        let path = dir.path.join("target").join("burdin").join("debug").join("test-failed.txt");
        FailedRecord::default().save(&path).unwrap();
        assert!(!path.exists());

        let record = FailedRecord { entries: vec![id("app", "boom")] };
        record.save(&path).unwrap();
        assert_eq!(FailedRecord::load(&path), Some(record));
        // Once a record exists, an empty one replaces it.
        FailedRecord::default().save(&path).unwrap();
        assert_eq!(FailedRecord::load(&path), Some(FailedRecord::default()));
    }
}
