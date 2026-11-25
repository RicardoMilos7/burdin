//! What `burdin test` prints and writes: the human report, `--list`, the JSON event stream, and the JUnit XML file
//! (`spec/test-runner.md`, "Output and exit status", "Listing tests", and "Machine-readable reports").

use super::{Attrs, Outcome, Plan, Planned, TestCase};
use burdin_diag::json_escape;
use std::path::Path;
use std::time::Duration;

/// The version of the JSON schema: the line formats of `--list --format json` and of the event stream. It is the
/// `version` of the `suite_started` event and changes only when an existing field changes meaning.
pub(super) const SCHEMA_VERSION: u32 = 1;

/// The counts a run ends with.
pub(super) struct Summary {
    pub(super) passed: usize,
    pub(super) failed: usize,
    pub(super) ignored: usize,
    pub(super) filtered_out: usize,
    pub(super) duration: Duration,
    /// The tests that ran for at least the slow threshold: the index into the plan and the milliseconds they took.
    pub(super) slow: Vec<(usize, u128)>,
}

/// Counts the outcomes. `slow_ms` is the threshold; `0` lists no test.
pub(super) fn summarize(plan: &Plan, outcomes: &[Option<Outcome>], slow_ms: u64, duration: Duration) -> Summary {
    let mut summary = Summary {
        passed: 0,
        failed: 0,
        ignored: plan.ignored,
        filtered_out: plan.filtered_out,
        duration,
        slow: vec![],
    };
    for (index, outcome) in outcomes.iter().enumerate() {
        let Some(outcome) = outcome else { continue };
        if outcome.passed {
            summary.passed += 1;
        } else {
            summary.failed += 1;
        }
        let ms = outcome.duration.as_millis();
        if slow_ms > 0 && ms >= u128::from(slow_ms) {
            summary.slow.push((index, ms));
        }
    }
    summary
}

// --- the human report --------------------------------------------------------------------------------------------

/// Prints the report in declaration order: the failures (and with `verbose` every test's output), the slow tests, and
/// the summary line. It runs after every test has finished, so the output is deterministic.
pub(super) fn print_human(plan: &Plan, outcomes: &[Option<Outcome>], summary: &Summary, verbose: bool) {
    for (index, planned) in plan.tests.iter().enumerate() {
        let case = &planned.case;
        let display = &case.display;
        // A doc test reports the doc comment it came from: the file and the line its block starts at.
        let label = if case.attrs.doc { "doc test" } else { "test" };
        let at = case.attrs.doc.then(|| format!("{}:{}", case.file, case.line));
        if planned.ignored {
            if verbose {
                println!("IGNORED {display}");
            }
            continue;
        }
        let outcome = outcomes[index].as_ref().expect("every runnable test has an outcome");
        if outcome.passed {
            if verbose {
                print_output(label, display, outcome, at.as_deref());
                println!("PASS {display}");
            }
        } else {
            print_failure(label, display, outcome, at.as_deref());
            println!("FAIL {display}");
        }
    }
    for &(index, ms) in &summary.slow {
        println!("slow test: {} ({ms} ms)", plan.tests[index].case.display);
    }
    println!("{}", summary_line(summary));
}

fn summary_line(summary: &Summary) -> String {
    let mut line = format!("test result: {} passed, {} failed", summary.passed, summary.failed);
    if summary.ignored > 0 {
        line.push_str(&format!(", {} ignored", summary.ignored));
    }
    if summary.filtered_out > 0 {
        line.push_str(&format!(", {} filtered out", summary.filtered_out));
    }
    line
}

fn print_failure(label: &str, display: &str, outcome: &Outcome, at: Option<&str>) {
    let reason = outcome.note.as_deref().filter(|note| !note.is_empty());
    // A doc test's heading carries its location; either heading carries its reason when it has one.
    match (at, reason) {
        (Some(at), Some(reason)) => println!("---- {label} \"{display}\" at {at}: {reason} ----"),
        (Some(at), None) => println!("---- {label} \"{display}\" at {at} ----"),
        (None, Some(reason)) => println!("---- {label} \"{display}\": {reason} ----"),
        (None, None) => {}
    }
    print_output(label, display, outcome, at);
}

fn print_output(label: &str, display: &str, outcome: &Outcome, at: Option<&str>) {
    print_stream(label, display, "stdout", &outcome.stdout, at);
    print_stream(label, display, "stderr", &outcome.stderr, at);
}

fn print_stream(label: &str, display: &str, stream: &str, bytes: &[u8], at: Option<&str>) {
    if bytes.is_empty() {
        return;
    }
    // A captured stream of a doc test is identified the same way its failure heading is.
    match at {
        Some(at) => println!("---- {label} \"{display}\" at {at} {stream} ----"),
        None => println!("---- {label} \"{display}\" {stream} ----"),
    }
    print!("{}", String::from_utf8_lossy(bytes));
    if !bytes.ends_with(b"\n") {
        println!();
    }
}

// --- --list -------------------------------------------------------------------------------------------------------

/// The attributes of a test as they are written in the source, in a fixed order (`[ignore, timeout(500)]`); empty
/// when the test has none. A doc test lists the attributes of its doc block (`[no_run]`).
pub(super) fn attribute_text(attrs: &Attrs) -> String {
    if attrs.doc {
        let mut parts = Vec::new();
        if attrs.ignore {
            parts.push("ignore".to_string());
        }
        if attrs.no_run {
            parts.push("no_run".to_string());
        }
        if attrs.should_panic {
            parts.push("should_panic".to_string());
        }
        if attrs.compile_fail {
            match &attrs.compile_fail_code {
                Some(code) => parts.push(format!("compile_fail {code}")),
                None => parts.push("compile_fail".to_string()),
            }
        }
        if parts.is_empty() { String::new() } else { format!("[{}]", parts.join(", ")) }
    } else {
        let mut parts = Vec::new();
        if attrs.ignore {
            parts.push("ignore".to_string());
        }
        if let Some(ms) = attrs.timeout_ms {
            parts.push(format!("timeout({ms})"));
        }
        if attrs.should_panic {
            match &attrs.should_panic_message {
                Some(message) => parts.push(format!("should_panic(\"{}\")", json_escape(message))),
                None => parts.push("should_panic".to_string()),
            }
        }
        if attrs.should_fail {
            parts.push("should_fail".to_string());
        }
        if parts.is_empty() { String::new() } else { format!("[{}]", parts.join(", ")) }
    }
}

/// `--list`: one line per selected test (its full path and attributes) and a count.
pub(super) fn print_list_human(plan: &Plan) {
    for planned in &plan.tests {
        let attributes = attribute_text(&planned.case.attrs);
        if attributes.is_empty() {
            println!("{}", planned.case.path);
        } else {
            println!("{} {attributes}", planned.case.path);
        }
    }
    let count = plan.tests.len();
    let noun = if count == 1 { "test" } else { "tests" };
    if plan.filtered_out > 0 {
        println!("{count} {noun}, {} filtered out", plan.filtered_out);
    } else {
        println!("{count} {noun}");
    }
}

/// `--list --format json`: one object per selected test.
pub(super) fn print_list_json(plan: &Plan) {
    for planned in &plan.tests {
        println!("{{{},\"attributes\":{}}}", identity_json(&planned.case), attributes_json(&planned.case.attrs));
    }
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", json_escape(text))
}

/// The fields that name a test in every JSON line: its package, module, name, full path, file, and line.
fn identity_json(case: &TestCase) -> String {
    format!(
        "\"package\":{},\"module\":{},\"name\":{},\"path\":{},\"file\":{},\"line\":{}",
        quoted(&case.package),
        quoted(&case.module),
        quoted(&case.name),
        quoted(&case.path),
        quoted(&case.file),
        case.line
    )
}

fn attributes_json(attrs: &Attrs) -> String {
    if attrs.doc {
        // The attributes of a doc block; the `test`-block fields do not apply to one.
        let code = attrs.compile_fail_code.as_deref().map_or("null".to_string(), quoted);
        return format!(
            "{{\"ignore\":{},\"should_panic\":{},\"no_run\":{},\"compile_fail\":{},\"compile_fail_code\":{code}}}",
            attrs.ignore, attrs.should_panic, attrs.no_run, attrs.compile_fail
        );
    }
    let timeout = attrs.timeout_ms.map_or("null".to_string(), |ms| ms.to_string());
    let message = attrs.should_panic_message.as_deref().map_or("null".to_string(), quoted);
    format!(
        "{{\"ignore\":{},\"timeout_ms\":{timeout},\"should_panic\":{},\"should_panic_message\":{message},\
         \"should_fail\":{}}}",
        attrs.ignore, attrs.should_panic, attrs.should_fail
    )
}

// --- the JSON event stream ----------------------------------------------------------------------------------------

/// `--format json`: one JSON object per line on standard output as the run progresses. Each event is one `println!`,
/// which holds the stdout lock for the whole line, so lines from concurrent tests never interleave.
pub(super) struct Stream {
    /// Passing tests carry their captured output only with `--verbose`, like the human report.
    pub(super) verbose: bool,
}

impl Stream {
    pub(super) fn suite_started(&self, plan: &Plan, packages: &[String]) {
        let names: Vec<String> = packages.iter().map(|name| quoted(name)).collect();
        println!(
            "{{\"event\":\"suite_started\",\"version\":{SCHEMA_VERSION},\"packages\":[{}],\"test_count\":{},\
             \"ignored\":{},\"filtered_out\":{}}}",
            names.join(","),
            plan.tests.len(),
            plan.ignored,
            plan.filtered_out
        );
    }

    pub(super) fn test_ignored(&self, planned: &Planned) {
        println!("{{\"event\":\"test_ignored\",{}}}", identity_json(&planned.case));
    }

    pub(super) fn test_started(&self, planned: &Planned) {
        println!("{{\"event\":\"test_started\",{}}}", identity_json(&planned.case));
    }

    pub(super) fn test_finished(&self, planned: &Planned, outcome: &Outcome) {
        let identity = identity_json(&planned.case);
        let duration = outcome.duration.as_millis();
        let stdout = quoted(&String::from_utf8_lossy(&outcome.stdout));
        let stderr = quoted(&String::from_utf8_lossy(&outcome.stderr));
        if outcome.passed {
            let output = if self.verbose { format!(",\"stdout\":{stdout},\"stderr\":{stderr}") } else { String::new() };
            println!("{{\"event\":\"test_passed\",{identity},\"duration_ms\":{duration}{output}}}");
        } else {
            let reason = match outcome.note.as_deref() {
                Some(note) if !note.is_empty() => quoted(note),
                _ => "null".to_string(),
            };
            println!(
                "{{\"event\":\"test_failed\",{identity},\"duration_ms\":{duration},\"reason\":{reason},\
                 \"stdout\":{stdout},\"stderr\":{stderr}}}"
            );
        }
    }

    pub(super) fn suite_finished(&self, plan: &Plan, summary: &Summary) {
        let slow: Vec<String> = summary
            .slow
            .iter()
            .map(|&(index, ms)| format!("{{\"path\":{},\"duration_ms\":{ms}}}", quoted(&plan.tests[index].case.path)))
            .collect();
        println!(
            "{{\"event\":\"suite_finished\",\"passed\":{},\"failed\":{},\"ignored\":{},\"filtered_out\":{},\
             \"duration_ms\":{},\"slow\":[{}]}}",
            summary.passed,
            summary.failed,
            summary.ignored,
            summary.filtered_out,
            summary.duration.as_millis(),
            slow.join(",")
        );
    }
}

// --- JUnit XML ----------------------------------------------------------------------------------------------------

/// Escapes text for an XML attribute or element. A character XML 1.0 cannot carry at all (most control characters)
/// becomes U+FFFD, so a test that prints a stray byte cannot make the report unreadable.
pub(super) fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            ch if (ch as u32) < 0x20 || matches!(ch as u32, 0xfffe | 0xffff) => out.push('\u{fffd}'),
            ch => out.push(ch),
        }
    }
    out
}

fn seconds(duration: Duration) -> String {
    format!("{}.{:03}", duration.as_secs(), duration.subsec_millis())
}

/// The JUnit XML report: one `testsuite` per package, one `testcase` per selected test. A failed test carries a
/// `failure` with the explanation (or `test failed`) and its captured output; an ignored test is `skipped`.
pub(super) fn junit_xml(plan: &Plan, outcomes: &[Option<Outcome>], summary: &Summary, verbose: bool) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(&format!(
        "<testsuites name=\"burdin test\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{}\">\n",
        plan.tests.len(),
        summary.failed,
        summary.ignored,
        seconds(summary.duration)
    ));
    let mut start = 0;
    while start < plan.tests.len() {
        let package = &plan.tests[start].case.package;
        let end = plan.tests[start..]
            .iter()
            .position(|planned| &planned.case.package != package)
            .map_or(plan.tests.len(), |offset| start + offset);
        let range = start..end;
        let failures = range.clone().filter(|&i| outcomes[i].as_ref().is_some_and(|o| !o.passed)).count();
        let skipped = range.clone().filter(|&i| plan.tests[i].ignored).count();
        let time: Duration = range.clone().filter_map(|i| outcomes[i].as_ref()).map(|o| o.duration).sum();
        xml.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" skipped=\"{skipped}\" time=\"{}\">\n",
            xml_escape(package),
            end - start,
            seconds(time)
        ));
        for index in range {
            xml.push_str(&testcase_xml(&plan.tests[index], outcomes[index].as_ref(), verbose));
        }
        xml.push_str("  </testsuite>\n");
        start = end;
    }
    xml.push_str("</testsuites>\n");
    xml
}

fn testcase_xml(planned: &Planned, outcome: Option<&Outcome>, verbose: bool) -> String {
    let case = &planned.case;
    let classname =
        if case.module.is_empty() { case.package.clone() } else { format!("{}.{}", case.package, case.module) };
    let time = outcome.map_or_else(|| "0.000".to_string(), |o| seconds(o.duration));
    let open = format!(
        "    <testcase classname=\"{}\" name=\"{}\" file=\"{}\" line=\"{}\" time=\"{time}\"",
        xml_escape(&classname),
        xml_escape(&case.name),
        xml_escape(&case.file),
        case.line
    );
    let Some(outcome) = outcome else {
        return format!("{open}>\n      <skipped message=\"ignored\"/>\n    </testcase>\n");
    };
    let mut body = String::new();
    if !outcome.passed {
        let message = match outcome.note.as_deref() {
            Some(note) if !note.is_empty() => note,
            _ => "test failed",
        };
        body.push_str(&format!(
            "      <failure message=\"{}\" type=\"TestFailure\">{}</failure>\n",
            xml_escape(message),
            xml_escape(&String::from_utf8_lossy(&outcome.stderr))
        ));
    }
    if !outcome.passed || verbose {
        if !outcome.stdout.is_empty() {
            body.push_str(&format!(
                "      <system-out>{}</system-out>\n",
                xml_escape(&String::from_utf8_lossy(&outcome.stdout))
            ));
        }
        if !outcome.stderr.is_empty() {
            body.push_str(&format!(
                "      <system-err>{}</system-err>\n",
                xml_escape(&String::from_utf8_lossy(&outcome.stderr))
            ));
        }
    }
    if body.is_empty() { format!("{open}/>\n") } else { format!("{open}>\n{body}    </testcase>\n") }
}

/// Writes the JUnit report to `path`.
pub(super) fn write_junit(
    path: &Path,
    plan: &Plan,
    outcomes: &[Option<Outcome>],
    summary: &Summary,
    verbose: bool,
) -> Result<(), String> {
    std::fs::write(path, junit_xml(plan, outcomes, summary, verbose))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_runner::Run;
    use std::path::PathBuf;

    fn planned(package: &str, module: &str, name: &str, attrs: Attrs, ignored: bool) -> Planned {
        let local = if module.is_empty() { name.to_string() } else { format!("{module}.{name}") };
        Planned {
            case: TestCase {
                package: package.to_string(),
                module: module.to_string(),
                name: name.to_string(),
                local: local.clone(),
                display: name.to_string(),
                path: local,
                file: "src/main.bd".to_string(),
                line: 7,
                run: Run::Block { exe: PathBuf::from("bin"), index: 0 },
                attrs,
            },
            ignored,
            timeout: None,
        }
    }

    fn outcome(passed: bool, ms: u64, note: Option<&str>) -> Outcome {
        Outcome {
            passed,
            stdout: b"out <1>\n".to_vec(),
            stderr: b"err & more\n".to_vec(),
            note: note.map(str::to_string),
            duration: Duration::from_millis(ms),
        }
    }

    #[test]
    fn attributes_print_in_source_form() {
        assert_eq!(attribute_text(&Attrs::default()), "");
        let attrs = Attrs {
            ignore: true,
            timeout_ms: Some(500),
            should_panic: true,
            should_panic_message: Some("boom \"x\"".to_string()),
            ..Attrs::default()
        };
        assert_eq!(attribute_text(&attrs), "[ignore, timeout(500), should_panic(\"boom \\\"x\\\"\")]");
        let attrs = Attrs { should_fail: true, ..Attrs::default() };
        assert_eq!(attribute_text(&attrs), "[should_fail]");
        // A doc block lists its own attributes, in the order they are written in the spec.
        let attrs = Attrs { doc: true, ignore: true, no_run: true, ..Attrs::default() };
        assert_eq!(attribute_text(&attrs), "[ignore, no_run]");
        let attrs = Attrs {
            doc: true,
            should_panic: true,
            compile_fail: true,
            compile_fail_code: Some("E0123".to_string()),
            ..Attrs::default()
        };
        assert_eq!(attribute_text(&attrs), "[should_panic, compile_fail E0123]");
    }

    #[test]
    fn attributes_json_lists_every_attribute() {
        let attrs = Attrs { timeout_ms: Some(0), should_fail: true, ..Attrs::default() };
        assert_eq!(
            attributes_json(&attrs),
            "{\"ignore\":false,\"timeout_ms\":0,\"should_panic\":false,\"should_panic_message\":null,\
             \"should_fail\":true}"
        );
        let attrs = Attrs { doc: true, no_run: true, compile_fail: true, ..Attrs::default() };
        assert_eq!(
            attributes_json(&attrs),
            "{\"ignore\":false,\"should_panic\":false,\"no_run\":true,\"compile_fail\":true,\"compile_fail_code\":null}"
        );
    }

    #[test]
    fn xml_escaping_covers_markup_and_invalid_characters() {
        assert_eq!(xml_escape("a<b>&\"c'\n"), "a&lt;b&gt;&amp;&quot;c&apos;\n");
        assert_eq!(xml_escape("bell\u{7}"), "bell\u{fffd}");
    }

    #[test]
    fn the_summary_line_keeps_the_established_format() {
        let mut summary =
            Summary { passed: 2, failed: 1, ignored: 0, filtered_out: 0, duration: Duration::ZERO, slow: vec![] };
        assert_eq!(summary_line(&summary), "test result: 2 passed, 1 failed");
        summary.ignored = 3;
        assert_eq!(summary_line(&summary), "test result: 2 passed, 1 failed, 3 ignored");
        summary.filtered_out = 4;
        assert_eq!(summary_line(&summary), "test result: 2 passed, 1 failed, 3 ignored, 4 filtered out");
    }

    #[test]
    fn slow_tests_are_listed_from_the_threshold() {
        let plan = Plan {
            tests: vec![
                planned("app", "", "quick", Attrs::default(), false),
                planned("app", "", "slow", Attrs::default(), false),
            ],
            ignored: 0,
            filtered_out: 0,
        };
        let outcomes = vec![Some(outcome(true, 10, None)), Some(outcome(true, 250, None))];
        let summary = summarize(&plan, &outcomes, 100, Duration::from_secs(1));
        assert_eq!(summary.slow, [(1, 250)]);
        assert!(summarize(&plan, &outcomes, 0, Duration::from_secs(1)).slow.is_empty());
        assert_eq!(summarize(&plan, &outcomes, 10, Duration::from_secs(1)).slow.len(), 2);
    }

    #[test]
    fn junit_groups_by_package_and_marks_failures_and_skips() {
        let plan = Plan {
            tests: vec![
                planned("app", "geometry", "area", Attrs::default(), false),
                planned("app", "", "boom", Attrs::default(), false),
                planned("lib", "", "later", Attrs { ignore: true, ..Attrs::default() }, true),
            ],
            ignored: 1,
            filtered_out: 0,
        };
        let outcomes = vec![Some(outcome(true, 12, None)), Some(outcome(false, 1500, Some("timed out"))), None];
        let summary = summarize(&plan, &outcomes, 0, Duration::from_millis(2000));
        let xml = junit_xml(&plan, &outcomes, &summary, false);
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"burdin test\" tests=\"3\" failures=\"1\" skipped=\"1\" time=\"2.000\">"));
        assert!(xml.contains("<testsuite name=\"app\" tests=\"2\" failures=\"1\" skipped=\"0\" time=\"1.512\">"));
        assert!(xml.contains("<testsuite name=\"lib\" tests=\"1\" failures=\"0\" skipped=\"1\" time=\"0.000\">"));
        assert!(xml.contains(
            "<testcase classname=\"app.geometry\" name=\"area\" file=\"src/main.bd\" line=\"7\" time=\"0.012\"/>"
        ));
        assert!(xml.contains("<failure message=\"timed out\" type=\"TestFailure\">err &amp; more\n</failure>"));
        assert!(xml.contains("<system-out>out &lt;1&gt;\n</system-out>"));
        assert!(xml.contains("<skipped message=\"ignored\"/>"));
    }
}
