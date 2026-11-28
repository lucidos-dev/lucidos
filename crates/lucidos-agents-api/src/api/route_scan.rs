use std::collections::BTreeSet;
use std::path::PathBuf;

pub(crate) struct RouteHit {
    pub file: String,
    pub func: String,
    pub path: String,
    pub methods: BTreeSet<String>,
}

const VERBS: &[&str] = &["get", "post", "put", "delete", "patch"];

fn api_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api")
}

pub(crate) fn scan_api_routes() -> Vec<RouteHit> {
    let mut found = Vec::new();
    let mut stack = vec![api_dir()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).expect("api dir is readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            if name == "route_scan.rs" {
                continue;
            }
            let label = path
                .strip_prefix(api_dir())
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let src = std::fs::read_to_string(&path).expect("source is readable");
            collect_routes(&src, &label, &mut found);
        }
    }
    found
}

fn collect_routes(src: &str, file: &str, found: &mut Vec<RouteHit>) {
    let mut cursor = 0usize;
    let mut func: Option<String> = None;
    while let Some(rel) = src[cursor..].find(".route(") {
        let at = cursor + rel;
        if let Some(name) = last_fn_name(&src[cursor..at]) {
            func = Some(name);
        }
        let Some(func) = func.clone() else {
            cursor = at + ".route(".len();
            continue;
        };
        let after = &src[at + ".route(".len()..];
        let Some(body) = balanced(after) else {
            cursor = at + ".route(".len();
            continue;
        };
        cursor = at + ".route(".len() + body.len();
        let Some(path) = first_string_literal(body) else {
            found.push(RouteHit {
                file: file.to_string(),
                func,
                path: String::new(),
                methods: BTreeSet::new(),
            });
            continue;
        };
        let mut methods: BTreeSet<String> = VERBS
            .iter()
            .filter(|v| mentions_call(body, v))
            .map(|v| v.to_uppercase())
            .collect();
        if mentions_call(body, "any") {
            methods.insert("ANY".to_string());
        }
        found.push(RouteHit {
            file: file.to_string(),
            func,
            path,
            methods,
        });
    }
}

fn last_fn_name(chunk: &str) -> Option<String> {
    let mut found = None;
    let mut rest = chunk;
    while let Some(at) = rest.find("fn ") {
        let before = rest[..at].chars().next_back();
        let after = &rest[at + 3..];
        if !before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                found = Some(name);
            }
        }
        rest = after;
    }
    found
}

fn balanced(after_open: &str) -> Option<&str> {
    let mut depth = 1usize;
    for (i, c) in after_open.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&after_open[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn first_string_literal(body: &str) -> Option<String> {
    let start = body.find('"')? + 1;
    let end = start + body[start..].find('"')?;
    Some(body[start..end].to_string())
}

fn mentions_call(body: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let abs = from + at;
        let preceding = body[..abs].chars().next_back();
        if !preceding.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        from = abs + needle.len();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_reads_the_real_router() {
        let hits = scan_api_routes();
        assert!(
            hits.len() > 150,
            "the scan found only {} registrations",
            hits.len()
        );
        let apply = hits
            .iter()
            .find(|h| h.path == "/changes/:id/apply")
            .expect("a known route");
        assert_eq!(apply.func, "router");
        assert!(apply.methods.contains("POST"));
    }

    #[test]
    fn a_route_names_the_function_that_registered_it() {
        let hits = scan_api_routes();
        let ui: Vec<&RouteHit> = hits.iter().filter(|h| h.func == "ui_router").collect();
        assert!(
            !ui.is_empty(),
            "apps::ui_router registers the app-asset routes and the scan lost them"
        );
        assert!(ui.iter().all(|h| h.file == "apps.rs"));
    }

    #[test]
    fn a_computed_path_is_recorded_with_no_literal() {
        let src =
            "pub fn router() -> Router {\n    Router::new().route(&built_path(), get(serve))\n}\n";
        let mut hits = Vec::new();
        collect_routes(src, "computed.rs", &mut hits);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.is_empty());
        assert_eq!(hits[0].func, "router");
    }

    #[test]
    fn a_method_name_is_matched_as_a_whole_word() {
        assert!(mentions_call("get(list).post(create)", "post"));
        assert!(!mentions_call("get(mcp_post)", "post"));
        assert!(mentions_call("any(forward)", "any"));
    }

    #[test]
    fn a_route_call_is_bounded_by_its_own_paren() {
        let src = r#"fn router() -> X { Router::new().route("/a", get(a)).route("/b", post(b)) }"#;
        let mut found = Vec::new();
        collect_routes(src, "x.rs", &mut found);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].path, "/a");
        assert_eq!(
            found[0].methods.iter().cloned().collect::<Vec<_>>(),
            ["GET"]
        );
        assert_eq!(found[1].path, "/b");
        assert_eq!(
            found[1].methods.iter().cloned().collect::<Vec<_>>(),
            ["POST"]
        );
    }
}
