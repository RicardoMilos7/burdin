//! `burdin bloat` (`spec/inspect.md`, INS16-INS19): where a binary's code comes from.
//!
//! The input is the symbol table of the program's object file ([`crate::object`]) and a table that says what each
//! Burdin symbol is: which function or closure, in which package and module, and which generic it instantiates. The
//! analysis groups the sizes five ways: the largest functions, the size per package and per module, the size per
//! generic with the instances added up, and the three kinds of code that belong to no source function: the shared
//! drop and clone glue (issue #426), the runtime, and anything else.

use crate::object::Symbol;
use burdin_diag::Json;
use std::collections::HashMap;

/// The package and module a function is written in. The module is empty for a one-file target.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Owner {
    pub package: String,
    pub module: String,
}

impl Owner {
    fn module_label(&self) -> String {
        if self.module.is_empty() {
            format!("{} (root)", self.package)
        } else {
            format!("{} {}", self.package, self.module)
        }
    }
}

/// What a Burdin symbol is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    /// The function's name as the program spells it (`largest<i32>`, `std.fmt.Formatter.pad_text`).
    pub name: String,
    pub owner: Owner,
    /// The generic part of an instance's name, `None` for a function that is not an instance.
    pub generic: Option<String>,
    /// A closure body rather than a named function.
    pub closure: bool,
}

/// Symbol name to what it is.
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub known: HashMap<String, Known>,
}

/// One function or closure with its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub size: u64,
    pub owner: Owner,
    pub closure: bool,
}

/// A count and the bytes they add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Total {
    pub count: usize,
    pub size: u64,
}

impl Total {
    fn add(&mut self, size: u64) {
        self.count += 1;
        self.size += size;
    }
}

/// The instances of one generic function that made it into the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericRow {
    pub generic: String,
    pub instances: usize,
    pub size: u64,
    /// The largest instance.
    pub largest: Item,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The size of the linked executable, when there is one.
    pub executable: Option<u64>,
    /// Everything in code sections: the functions below, the glue, the runtime, and the rest.
    pub code: Total,
    /// Every data symbol (constants, vtables, tables).
    pub data: Total,
    /// The largest functions and closures, at most the requested number.
    pub largest: Vec<Item>,
    /// How many functions and closures there are in all.
    pub functions: usize,
    pub packages: Vec<(String, Total)>,
    pub modules: Vec<(Owner, Total)>,
    pub generics: Vec<GenericRow>,
    /// The drop and clone glue shared between types (`burdin.drop.N`, `burdin.clone.N`).
    pub glue: Total,
    /// The runtime: output, strings, allocation, panics, the entry point.
    pub runtime: Total,
    /// Code no source function accounts for.
    pub other: Total,
}

/// Strips the prefix a split build gives the symbols it had to make visible between codegen units.
fn base_name(symbol: &str) -> &str {
    symbol
        .strip_prefix("burdin.cgu.")
        .map(|rest| rest.strip_prefix("burdin.").map_or(symbol, |_| rest))
        .unwrap_or(symbol)
}

fn is_glue(name: &str) -> bool {
    ["burdin.drop.", "burdin.clone."]
        .iter()
        .any(|prefix| name.strip_prefix(prefix).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())))
}

fn is_runtime(name: &str) -> bool {
    name.starts_with("burdin_rt_") || name.starts_with("burdin_panic") || name == "main"
}

/// Groups the symbols of one object file. `top` limits the list of largest functions and of generics; the package
/// and module lists are complete.
pub fn analyze(symbols: &[Symbol], table: &Table, executable: Option<u64>, top: usize) -> Report {
    let mut report = Report { executable, ..Report::default() };
    let mut items: Vec<Item> = Vec::new();
    for symbol in symbols {
        if !symbol.function {
            report.data.add(symbol.size);
            continue;
        }
        report.code.add(symbol.size);
        let name = base_name(&symbol.name);
        if let Some(known) = table.known.get(name).or_else(|| table.known.get(&symbol.name)) {
            items.push(Item {
                name: known.name.clone(),
                size: symbol.size,
                owner: known.owner.clone(),
                closure: known.closure,
            });
        } else if is_glue(name) {
            report.glue.add(symbol.size);
        } else if is_runtime(name) {
            report.runtime.add(symbol.size);
        } else {
            report.other.add(symbol.size);
        }
    }
    report.functions = items.len();

    let mut packages: HashMap<&str, Total> = HashMap::new();
    let mut modules: HashMap<&Owner, Total> = HashMap::new();
    for item in &items {
        packages.entry(&item.owner.package).or_default().add(item.size);
        modules.entry(&item.owner).or_default().add(item.size);
    }
    report.packages = packages.into_iter().map(|(name, total)| (name.to_string(), total)).collect();
    report.modules = modules.into_iter().map(|(owner, total)| (owner.clone(), total)).collect();
    // The biggest first; equal sizes fall back to the name so the order never depends on hashing.
    report.packages.sort_by(|a, b| b.1.size.cmp(&a.1.size).then_with(|| a.0.cmp(&b.0)));
    report.modules.sort_by(|a, b| b.1.size.cmp(&a.1.size).then_with(|| a.0.cmp(&b.0)));

    let mut generics: HashMap<String, GenericRow> = HashMap::new();
    let generic_of: HashMap<&str, &str> =
        table.known.values().filter_map(|k| k.generic.as_deref().map(|g| (k.name.as_str(), g))).collect();
    for item in &items {
        let Some(&generic) = generic_of.get(item.name.as_str()) else { continue };
        let row = generics.entry(generic.to_string()).or_insert_with(|| GenericRow {
            generic: generic.to_string(),
            instances: 0,
            size: 0,
            largest: item.clone(),
        });
        row.instances += 1;
        row.size += item.size;
        if item.size > row.largest.size {
            row.largest = item.clone();
        }
    }
    report.generics = generics.into_values().collect();
    report.generics.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.generic.cmp(&b.generic)));
    report.generics.truncate(top);

    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    items.truncate(top);
    report.largest = items;
    report
}

fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 { "0.0".to_string() } else { format!("{:.1}", part as f64 * 100.0 / whole as f64) }
}

/// The report as text. `title` names the program and the profile.
pub fn render_human(report: &Report, title: &str) -> String {
    let code = report.code.size;
    let mut out = format!("bloat: {title}\n");
    if let Some(size) = report.executable {
        out.push_str(&format!("executable   {:>12} bytes\n", grouped(size)));
    }
    out.push_str(&format!("code         {:>12} bytes in {} functions\n", grouped(code), report.code.count));
    out.push_str(&format!("data         {:>12} bytes in {} symbols\n", grouped(report.data.size), report.data.count));

    out.push_str(&format!("\nlargest functions (top {} of {})\n", report.largest.len(), report.functions));
    out.push_str("        size      %  function\n");
    for item in &report.largest {
        let kind = if item.closure { " (closure)" } else { "" };
        out.push_str(&format!(
            "  {:>10}  {:>5}  {}{kind}  [{}]\n",
            grouped(item.size),
            percent(item.size, code),
            item.name,
            item.owner.module_label()
        ));
    }

    out.push_str("\nsize per package\n        size      %  functions  package\n");
    for (package, total) in &report.packages {
        out.push_str(&format!(
            "  {:>10}  {:>5}  {:>9}  {package}\n",
            grouped(total.size),
            percent(total.size, code),
            total.count
        ));
    }
    out.push_str("\nsize per module\n        size      %  functions  module\n");
    for (owner, total) in &report.modules {
        out.push_str(&format!(
            "  {:>10}  {:>5}  {:>9}  {}\n",
            grouped(total.size),
            percent(total.size, code),
            total.count,
            owner.module_label()
        ));
    }
    if !report.generics.is_empty() {
        out.push_str("\nsize per generic function (instances added up)\n        size      %  instances  generic\n");
        for row in &report.generics {
            out.push_str(&format!(
                "  {:>10}  {:>5}  {:>9}  {}  (largest: {}, {} bytes)\n",
                grouped(row.size),
                percent(row.size, code),
                row.instances,
                row.generic,
                row.largest.name,
                grouped(row.largest.size)
            ));
        }
    }
    out.push_str("\ncode that is not a source function\n");
    for (label, total) in
        [("shared drop and clone glue", report.glue), ("runtime", report.runtime), ("other", report.other)]
    {
        out.push_str(&format!(
            "  {:>10}  {:>5}  {:>9}  {label}\n",
            grouped(total.size),
            percent(total.size, code),
            total.count
        ));
    }
    out
}

fn count(n: u64) -> Json {
    Json::Num(n)
}

fn total_json(total: Total) -> Json {
    Json::obj([("count", count(total.count as u64)), ("size", count(total.size))])
}

fn item_json(item: &Item) -> Json {
    Json::obj([
        ("name", Json::str(&item.name)),
        ("size", count(item.size)),
        ("package", Json::str(&item.owner.package)),
        ("module", Json::str(&item.owner.module)),
        ("closure", Json::Bool(item.closure)),
    ])
}

/// The report as one JSON object on one line (`"version": 1`).
pub fn render_json(report: &Report, title: &str) -> String {
    let json = Json::obj([
        ("version", count(1)),
        ("title", Json::str(title)),
        ("executable", report.executable.map_or(Json::Null, count)),
        ("code", total_json(report.code)),
        ("data", total_json(report.data)),
        ("functions", count(report.functions as u64)),
        ("largest", Json::Arr(report.largest.iter().map(item_json).collect())),
        (
            "packages",
            Json::Arr(
                report
                    .packages
                    .iter()
                    .map(|(name, total)| {
                        Json::obj([
                            ("package", Json::str(name)),
                            ("count", count(total.count as u64)),
                            ("size", count(total.size)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "modules",
            Json::Arr(
                report
                    .modules
                    .iter()
                    .map(|(owner, total)| {
                        Json::obj([
                            ("package", Json::str(&owner.package)),
                            ("module", Json::str(&owner.module)),
                            ("count", count(total.count as u64)),
                            ("size", count(total.size)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "generics",
            Json::Arr(
                report
                    .generics
                    .iter()
                    .map(|row| {
                        Json::obj([
                            ("generic", Json::str(&row.generic)),
                            ("instances", count(row.instances as u64)),
                            ("size", count(row.size)),
                            ("largest", item_json(&row.largest)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("glue", total_json(report.glue)),
        ("runtime", total_json(report.runtime)),
        ("other", total_json(report.other)),
    ]);
    let mut out = String::new();
    json.write(&mut out);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(package: &str, module: &str) -> Owner {
        Owner { package: package.to_string(), module: module.to_string() }
    }

    fn known(name: &str, package: &str, module: &str, generic: Option<&str>) -> (String, Known) {
        (
            format!("burdin.{name}"),
            Known {
                name: name.to_string(),
                owner: owner(package, module),
                generic: generic.map(str::to_string),
                closure: false,
            },
        )
    }

    fn function(name: &str, size: u64) -> Symbol {
        Symbol { name: name.to_string(), size, function: true }
    }

    fn sample() -> (Vec<Symbol>, Table) {
        let table = Table {
            known: [
                known("main", "app", "main", None),
                known("push<i32>", "app", "main", Some("push")),
                known("push<String>", "app", "main", Some("push")),
                known("std.fmt.pad", "std", "std.fmt", None),
            ]
            .into_iter()
            .collect(),
        };
        let symbols = vec![
            function("burdin.main", 100),
            function("burdin.push<i32>", 40),
            function("burdin.push<String>", 60),
            function("burdin.std.fmt.pad", 30),
            function("burdin.drop.3", 20),
            function("burdin.clone.4", 10),
            function("burdin_rt_print", 25),
            function("main", 5),
            function("mystery", 7),
            Symbol { name: "burdin.vtable.0".to_string(), size: 24, function: false },
        ];
        (symbols, table)
    }

    #[test]
    fn symbols_are_grouped_by_package_module_generic_and_kind() {
        let (symbols, table) = sample();
        let report = analyze(&symbols, &table, Some(1000), 3);
        assert_eq!(report.code, Total { count: 9, size: 297 });
        assert_eq!(report.data, Total { count: 1, size: 24 });
        assert_eq!(report.functions, 4);
        assert_eq!(
            report.largest.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["main", "push<String>", "push<i32>"]
        );
        assert_eq!(report.packages[0], ("app".to_string(), Total { count: 3, size: 200 }));
        assert_eq!(report.packages[1], ("std".to_string(), Total { count: 1, size: 30 }));
        assert_eq!(report.modules[0].0, owner("app", "main"));
        assert_eq!(report.generics.len(), 1);
        assert_eq!((report.generics[0].instances, report.generics[0].size), (2, 100));
        assert_eq!(report.generics[0].largest.name, "push<String>");
        assert_eq!(report.glue, Total { count: 2, size: 30 });
        assert_eq!(report.runtime, Total { count: 2, size: 30 });
        assert_eq!(report.other, Total { count: 1, size: 7 });
    }

    #[test]
    fn units_of_a_split_build_are_recognized_by_their_original_names() {
        let table = Table { known: [known("helper", "app", "main", None)].into_iter().collect() };
        let symbols = vec![function("burdin.cgu.burdin.helper", 12), function("burdin.cgu.burdin.drop.1", 8)];
        let report = analyze(&symbols, &table, None, 5);
        assert_eq!(report.packages, vec![("app".to_string(), Total { count: 1, size: 12 })]);
        assert_eq!(report.glue, Total { count: 1, size: 8 });
    }

    #[test]
    fn the_text_report_has_a_section_per_grouping() {
        let (symbols, table) = sample();
        let text = render_human(&analyze(&symbols, &table, Some(1000), 10), "app (release)");
        for heading in [
            "bloat: app (release)",
            "executable",
            "largest functions (top 4 of 4)",
            "size per package",
            "size per module",
            "size per generic function",
            "shared drop and clone glue",
        ] {
            assert!(text.contains(heading), "missing `{heading}`:\n{text}");
        }
        assert!(text.contains("push  (largest: push<String>, 60 bytes)"), "{text}");
        let json = render_json(&analyze(&symbols, &table, None, 10), "app");
        assert!(json.starts_with("{\"version\":1,\"title\":\"app\",\"executable\":null,"), "{json}");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1234567), "1,234,567");
        assert_eq!(percent(1, 3), "33.3");
        assert_eq!(percent(0, 0), "0.0");
    }
}
