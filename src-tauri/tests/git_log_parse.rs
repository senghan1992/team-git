use git_companion::git::log::parse_log;

const SEP: char = '\x1f';

fn row(sha: &str, msg: &str, author: &str, date: &str, parents: &str) -> String {
    format!("{sha}{SEP}{msg}{SEP}{author}{SEP}{date}{SEP}{parents}\n")
}

#[test]
fn single_commit_no_parents() {
    let s = row(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "initial",
        "Alice",
        "2024-01-02T03:04:05+00:00",
        "",
    );
    let c = parse_log(&s).unwrap();
    assert_eq!(c.len(), 1);
    assert!(c[0].parents.is_empty());
    assert_eq!(c[0].message, "initial");
}

#[test]
fn merge_commit_with_two_parents() {
    let s = row(
        "1111111111111111111111111111111111111111",
        "merge feat",
        "Bob",
        "2024-01-02T03:04:05+00:00",
        "2222222222222222222222222222222222222222 3333333333333333333333333333333333333333",
    );
    let c = parse_log(&s).unwrap();
    assert_eq!(c[0].parents.len(), 2);
}

#[test]
fn multiple_lines_preserve_order() {
    let s = row(
        "a".repeat(40).as_str(),
        "first",
        "Alice",
        "2024-01-01T00:00:00+00:00",
        "",
    ) + &row(
        "b".repeat(40).as_str(),
        "second",
        "Bob",
        "2024-01-02T00:00:00+00:00",
        &"a".repeat(40),
    );
    let c = parse_log(&s).unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].message, "first");
    assert_eq!(c[1].message, "second");
    assert_eq!(c[1].parents, vec!["a".repeat(40)]);
}

#[test]
fn rejects_malformed_log_line() {
    let s = "garbage no separators here\n";
    assert!(parse_log(s).is_err());
}

#[test]
fn empty_input_yields_no_commits() {
    let c = parse_log("").unwrap();
    assert!(c.is_empty());
}
