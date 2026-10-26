//! linearctl stays generic (docs/design.md, "What belongs in linearctl"):
//! the source names no workspace and adds no hidden markers to text it
//! sends. Extend the list when a convention tries to creep in.

#[test]
fn no_workspace_conventions() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("generic.rs") {
                out.push(p);
            }
        }
    }
    let mut files = vec![];
    walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
    assert!(files.len() > 10);
    let banned = ["<!--", "agent-context", "manekineko", "teachify"];
    for f in files {
        for (i, line) in std::fs::read_to_string(&f).unwrap().lines().enumerate() {
            let l = line.to_lowercase();
            let word = |w: &str| {
                l.match_indices(w).any(|(at, _)| {
                    let before = l[..at].chars().last();
                    let after = l[at + w.len()..].chars().next();
                    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '_');
                    edge(before) && edge(after)
                })
            };
            let found = banned.iter().find(|b| l.contains(*b)).or(["kaik"].iter().find(|w| word(w)));
            assert!(
                found.is_none(),
                "{}:{}: {found:?} is a workspace convention, not Linear's API",
                f.display(),
                i + 1
            );
        }
    }
}
