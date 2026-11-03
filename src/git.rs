use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::glob::Glob;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Add,
    Modify,
    Delete,
}

pub struct Change {
    pub action: Action,
    pub path: String,
}

pub struct Commit {
    /// Index of its repository, when several are shown.
    pub repo: usize,
    pub sha: String,
    pub time: i64,
    /// Author name, as written.
    pub author: String,
    /// Lowercased author email: what GitHub knows the author by.
    pub email: String,
    pub changes: Vec<Change>,
    /// For a side-branch commit (off HEAD's first-parent chain, the main line): the main-line
    /// merge that brings it in, when it comes later in the history (`--branches`).
    pub merged_by: Option<usize>,
    /// That merge's sha, until the histories are merged and its index is known.
    merge: Option<String>,
}

/// The history to play: `commits[..start]` are applied at once, before the animation begins.
pub struct Loaded {
    pub commits: Vec<Commit>,
    pub start: usize,
    /// (unix time, name) of every tag, oldest first.
    pub tags: Vec<(i64, String)>,
}

/// `--from` and `--to` as unix times. Resolved before the window opens, so a typo fails at once.
pub struct Range {
    from: Option<i64>,
    to: Option<i64>,
}

impl Range {
    pub fn new(repos: &[PathBuf], config: &Config) -> Result<Self, String> {
        // A commit or tag is looked up in each repository in turn.
        let resolve = |spec: &str, end_of_day| {
            let mut found = repos.iter().map(|repo| bound(repo, spec, end_of_day));
            let first = found.next().expect("there is always a repository");
            first.or_else(|e| found.find_map(Result::ok).ok_or(e))
        };
        Ok(Self {
            from: config
                .from
                .as_deref()
                .map(|spec| resolve(spec, false))
                .transpose()?,
            to: config
                .to
                .as_deref()
                .map(|spec| resolve(spec, true))
                .transpose()?,
        })
    }
}

/// Short names for the repositories (their folder names, made unique), when there are several.
pub fn repo_names(repos: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for repo in repos {
        let path = repo.canonicalize().unwrap_or_else(|_| repo.clone());
        let name = path
            .file_name()
            .map_or_else(|| "repo".to_string(), |n| n.to_string_lossy().into_owned());
        let mut unique = name.clone();
        for k in 2.. {
            if !names.contains(&unique) {
                break;
            }
            unique = format!("{name}-{k}");
        }
        names.push(unique);
    }
    names
}

/// Reads, filters and trims the history as the settings ask. Several repositories are read in
/// parallel and merged on one timeline, each under a top-level folder named after it.
pub fn history(repos: &[PathBuf], config: &Config, range: Range) -> Result<Loaded, String> {
    let Range { from, to } = range;
    let filter = Filter::new(config);
    let loaded: Vec<Result<Vec<Commit>, String>> = std::thread::scope(|scope| {
        let threads: Vec<_> = repos
            .iter()
            .map(|repo| scope.spawn(|| load(repo)))
            .collect();
        threads
            .into_iter()
            .map(|t| {
                t.join()
                    .unwrap_or_else(|_| Err("git log failed".to_string()))
            })
            .collect()
    });
    let names = repo_names(repos);
    let mut histories = Vec::new();
    for (i, commits) in loaded.into_iter().enumerate() {
        let mut commits = commits?;
        filter.apply(&mut commits);
        for commit in &mut commits {
            commit.repo = i;
            if repos.len() > 1 {
                for change in &mut commit.changes {
                    change.path = format!("{}/{}", names[i], change.path);
                }
            }
        }
        histories.push(commits);
    }
    let mut commits = merge(histories);
    if let Some(to) = to {
        commits.retain(|c| c.time <= to);
    }
    link_merges(&mut commits);
    if commits.is_empty() {
        let repos: Vec<String> = repos.iter().map(|r| r.display().to_string()).collect();
        return Err(format!("no commits to show in {}", repos.join(", ")));
    }
    let mut start = from.map_or(0, |from| commits.partition_point(|c| c.time < from));
    if start == commits.len() {
        return Err("no commits after --from".to_string());
    }
    if let Some(percent) = config.start {
        let (first, last) = (commits[start].time, commits[commits.len() - 1].time);
        let at = first + ((last - first) as f64 * percent / 100.0) as i64;
        start = commits
            .partition_point(|c| c.time < at)
            .min(commits.len() - 1);
    }
    let mut tags: Vec<(i64, String)> = Vec::new();
    for (i, repo) in repos.iter().enumerate() {
        let prefix = if repos.len() > 1 {
            format!("{} ", names[i])
        } else {
            String::new()
        };
        tags.extend(
            self::tags(repo)
                .into_iter()
                .map(|(t, name)| (t, format!("{prefix}{name}"))),
        );
    }
    tags.sort_by_key(|(t, _)| *t);
    Ok(Loaded {
        commits,
        start,
        tags,
    })
}

/// Merges histories by time, keeping each one's own order (author dates aren't always
/// increasing, and a repository's changes must apply in the order git gives them).
fn merge(histories: Vec<Vec<Commit>>) -> Vec<Commit> {
    let total = histories.iter().map(Vec::len).sum();
    let mut queues: Vec<std::collections::VecDeque<Commit>> =
        histories.into_iter().map(Into::into).collect();
    let mut merged = Vec::with_capacity(total);
    while let Some(q) = (0..queues.len())
        .filter(|&q| !queues[q].is_empty())
        .min_by_key(|&q| queues[q][0].time)
    {
        merged.extend(queues[q].pop_front());
    }
    merged
}

/// Turns each side-branch commit's merge sha into the merge's index, if it comes later (a merge
/// filtered out, or past `--to`, leaves its commits on the main line).
fn link_merges(commits: &mut [Commit]) {
    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.sha.as_str(), i))
        .collect();
    let merged: Vec<Option<usize>> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.merge
                .as_deref()
                .and_then(|sha| index.get(sha).copied())
                .filter(|&m| m > i)
        })
        .collect();
    for (commit, merged_by) in commits.iter_mut().zip(merged) {
        commit.merged_by = merged_by;
    }
}

/// Finds which main-line merge brings in each side-branch commit. The main line is HEAD's
/// first-parent chain (HEAD: the one commit that is nobody's parent). Going through it oldest
/// first, each merge claims the commits its other parents reach that nothing claimed before:
/// exactly what that merge brings. Works on the whole graph, before any filtering.
fn mark_side_branches(commits: &mut [Commit], parents: &[Vec<String>]) {
    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.sha.as_str(), i))
        .collect();
    let find = |sha: &String| index.get(sha.as_str()).copied();
    let mut has_child = vec![false; commits.len()];
    for p in parents.iter().flatten().filter_map(find) {
        has_child[p] = true;
    }
    let Some(head) = (0..commits.len()).rev().find(|&i| !has_child[i]) else {
        return;
    };
    // `None`: not reached yet; `Some(None)`: on the main line; `Some(Some(m))`: brought by `m`.
    let mut owner: Vec<Option<Option<usize>>> = vec![None; commits.len()];
    let mut main_line = Vec::new();
    let mut at = Some(head);
    while let Some(c) = at.filter(|&c| owner[c].is_none()) {
        owner[c] = Some(None);
        main_line.push(c);
        at = parents[c].first().and_then(find);
    }
    let mut stack = Vec::new();
    for &merge in main_line.iter().rev() {
        stack.extend(parents[merge].iter().skip(1).filter_map(find));
        while let Some(c) = stack.pop() {
            if owner[c].is_none() {
                owner[c] = Some(Some(merge));
                stack.extend(parents[c].iter().filter_map(find));
            }
        }
    }
    let shas: Vec<Option<String>> = owner
        .iter()
        .map(|o| o.flatten().map(|m| commits[m].sha.clone()))
        .collect();
    for (commit, sha) in commits.iter_mut().zip(shas) {
        commit.merge = sha;
    }
}

/// The repository's tags with their dates (the tagger's, or the commit's for lightweight tags).
/// Missing tags only cost the seek bar its ticks, so errors give an empty list.
fn tags(repo: &Path) -> Vec<(i64, String)> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "for-each-ref",
            "--sort=creatordate",
            "--format=%(creatordate:unix)%09%(refname:short)",
            "refs/tags",
        ])
        .output();
    let Ok(out) = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|line| {
            let (time, name) = line.split_once('\t')?;
            Some((time.parse().ok()?, name.to_string()))
        })
        .collect()
}

/// Path and author filters, applied once while loading so filtered things cost nothing later.
struct Filter {
    include: Vec<Glob>,
    exclude: Vec<Glob>,
    exclude_user: Vec<Glob>,
}

impl Filter {
    fn new(config: &Config) -> Self {
        let globs = |list: &[String]| list.iter().map(|p| Glob::new(p)).collect();
        // Authors are compared lowercased, like `Commit::author` and `Commit::email`.
        let users: Vec<String> = config
            .exclude_user
            .iter()
            .map(|u| u.to_lowercase())
            .collect();
        Self {
            include: globs(&config.include),
            exclude: globs(&config.exclude),
            exclude_user: globs(&users),
        }
    }

    fn keeps(&self, path: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|g| g.matches(path)))
            && !self.exclude.iter().any(|g| g.matches(path))
    }

    /// Drops excluded authors' commits, filtered files, and commits left with no files
    /// (commits that never had any, like merges, stay: their author is still at work).
    fn apply(&self, commits: &mut Vec<Commit>) {
        commits.retain_mut(|commit| {
            if self
                .exclude_user
                .iter()
                .any(|g| g.matches(&commit.author.to_lowercase()) || g.matches(&commit.email))
            {
                return false;
            }
            let had_changes = !commit.changes.is_empty();
            commit.changes.retain(|change| self.keeps(&change.path));
            !had_changes || !commit.changes.is_empty()
        });
    }
}

/// The unix time of a `--from`/`--to` bound: a date (YYYY-MM-DD, the whole day included)
/// or anything git can resolve to a commit (sha, tag, branch).
fn bound(repo: &Path, spec: &str, end_of_day: bool) -> Result<i64, String> {
    if let Some(day) = parse_date(spec) {
        return Ok(if end_of_day { day + 86_399 } else { day });
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "-1", "--format=%at", spec, "--"])
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let time = String::from_utf8_lossy(&out.stdout).trim().parse().ok();
    time.filter(|_| out.status.success()).ok_or_else(|| {
        format!("`{spec}` is neither a date (YYYY-MM-DD) nor a commit of this repository")
    })
}

/// Reads the whole history of `repo`, oldest commit first.
///
/// `git log --name-status` shows no changes for merge commits, so files a merge drops (a
/// branch's files left out when merging it) would never be deleted, and would linger to the
/// end. Merges therefore get the files they delete relative to any parent, read by a second
/// `git log` run in parallel.
fn load(repo: &Path) -> Result<Vec<Commit>, String> {
    // \x1e separates commits, \x1f separates fields: neither can appear in a name.
    let (log, merges) = std::thread::scope(|scope| {
        let merges = scope.spawn(|| {
            git_log(
                repo,
                &[
                    "--merges",
                    "-m",
                    "--diff-filter=D",
                    "--pretty=format:%x1e%H",
                ],
            )
        });
        (
            git_log(
                repo,
                &[
                    "--reverse",
                    "--pretty=format:%x1e%H%x1f%at%x1f%aN%x1f%aE%x1f%P",
                ],
            ),
            merges.join(),
        )
    });
    let (mut commits, parents): (Vec<Commit>, Vec<Vec<String>>) =
        log?.split('\x1e').filter_map(parse_commit).unzip();
    mark_side_branches(&mut commits, &parents);
    let merges = merges.map_err(|_| "git log failed".to_string())??;
    let mut dropped: HashMap<&str, Vec<&str>> = HashMap::new();
    for record in merges.split('\x1e') {
        let mut lines = record.lines();
        let Some(sha) = lines.next() else { continue };
        let paths = dropped.entry(sha).or_default();
        // One block per parent: the same file can be listed more than once.
        for path in lines.filter_map(|l| l.strip_prefix("D\t")) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    for commit in &mut commits {
        if let Some(paths) = dropped.get(commit.sha.as_str()) {
            commit.changes.extend(paths.iter().map(|p| Change {
                action: Action::Delete,
                path: p.to_string(),
            }));
        }
    }
    Ok(commits)
}

/// Runs `git log` with the options both reads share, plus `args`.
fn git_log(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "core.quotePath=false",
            "log",
            "--no-renames",
            "--name-status",
        ])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A commit and its parents' shas.
fn parse_commit(record: &str) -> Option<(Commit, Vec<String>)> {
    let mut lines = record.lines();
    let mut header = lines.next()?.split('\x1f');
    let (sha, time, author, email) = (
        header.next()?,
        header.next()?,
        header.next()?,
        header.next()?,
    );
    let parents = header
        .next()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let changes = lines
        .filter_map(|line| {
            let (status, path) = line.split_once('\t')?;
            let action = match status.chars().next()? {
                'A' => Action::Add,
                'D' => Action::Delete,
                _ => Action::Modify,
            };
            Some(Change {
                action,
                path: path.to_string(),
            })
        })
        .collect();
    let commit = Commit {
        repo: 0,
        sha: sha.to_string(),
        time: time.parse().ok()?,
        author: author.to_string(),
        email: email.to_lowercase(),
        changes,
        merged_by: None,
        merge: None,
    };
    Some((commit, parents))
}

/// Parses YYYY-MM-DD (UTC) into a unix timestamp, using Howard Hinnant's days-from-civil algorithm.
fn parse_date(text: &str) -> Option<i64> {
    let mut parts = text
        .trim()
        .splitn(3, '-')
        .map(|p| p.parse::<i64>().ok().filter(|_| p.len() >= 2));
    let (year, month, day) = (parts.next()??, parts.next()??, parts.next()??);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=12).contains(&month) || !(1..=days_in_month).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86400)
}

/// Formats a unix timestamp as YYYY-MM-DD (UTC), using Howard Hinnant's civil-from-days algorithm.
pub fn date(timestamp: i64) -> String {
    let z = timestamp.div_euclid(86400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        Action, Change, Commit, Config, Filter, Range, date, history, link_merges, load,
        mark_side_branches, merge, parse_commit, parse_date, repo_names, tags,
    };

    fn commit(sha: &str) -> Commit {
        let (author, email) = (String::new(), String::new());
        Commit {
            repo: 0,
            sha: sha.to_string(),
            time: 0,
            author,
            email,
            changes: Vec::new(),
            merged_by: None,
            merge: None,
        }
    }

    #[test]
    fn side_branches_belong_to_their_merge() {
        // a - b - d ------- g   main line (first parents)
        //  \     /         /
        //   c --+---- e - f    c merged by d; e and f (a merge of main into the branch) by g
        let graph = [
            ("a", ""),
            ("b", "a"),
            ("c", "a"),
            ("d", "b c"),
            ("e", "c"),
            ("f", "e d"),
            ("g", "d f"),
        ];
        let mut commits: Vec<Commit> = graph.iter().map(|(sha, _)| commit(sha)).collect();
        let parents: Vec<Vec<String>> = graph
            .iter()
            .map(|(_, p)| p.split_whitespace().map(str::to_string).collect())
            .collect();
        mark_side_branches(&mut commits, &parents);
        link_merges(&mut commits);
        let merged: Vec<Option<usize>> = commits.iter().map(|c| c.merged_by).collect();
        assert_eq!(merged, [None, None, Some(3), None, Some(6), Some(6), None]);
    }

    #[test]
    fn dates_round_trip() {
        for text in ["1970-01-01", "2000-02-29", "2024-12-31", "2026-03-01"] {
            assert_eq!(date(parse_date(text).unwrap()), text);
        }
        assert_eq!(parse_date("1970-01-02"), Some(86400));
        assert_eq!(parse_date("v1.0"), None);
        assert_eq!(parse_date("abc1234"), None);
    }

    #[test]
    fn dates_check_their_parts() {
        assert_eq!(parse_date(" 1970-01-01\n"), Some(0));
        assert_eq!(parse_date("1969-12-31"), Some(-DAY));
        assert_eq!(date(-1), "1969-12-31");
        assert_eq!(date(DAY - 1), "1970-01-01");
        assert_eq!(date(parse_date("1900-03-01").unwrap() - DAY), "1900-02-28");
        assert_eq!(date(parse_date("2000-03-01").unwrap() - DAY), "2000-02-29");
        for bad in [
            "2024-13-01",
            "2024-00-10",
            "2024-01-00",
            "2024-01-32",
            "2024-1-5",
        ] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        for bad in ["2024-01", "2024", "", "2024-01-01T10:00", "2024/01/01"] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates_past_the_end_of_the_month_are_refused() {
        assert_eq!(parse_date("2023-02-29"), None);
        assert_eq!(parse_date("2024-02-30"), None);
        assert_eq!(parse_date("2024-04-31"), None);
    }

    #[test]
    fn log_records_are_parsed() {
        let record = "abc\x1f1700000000\x1fAda Lovelace\x1fAda@Example.ORG\x1fp1 p2\n\
                      A\tsrc/new.rs\nM\tsrc/main.rs\nD\told.txt\nT\tlink\n\nnot a change";
        let (commit, parents) = parse_commit(record).unwrap();
        assert_eq!(commit.sha, "abc");
        assert_eq!(commit.time, 1_700_000_000);
        assert_eq!(commit.author, "Ada Lovelace");
        assert_eq!(commit.email, "ada@example.org");
        assert_eq!(parents, ["p1", "p2"]);
        let changes: Vec<(Action, &str)> = commit
            .changes
            .iter()
            .map(|c| (c.action, c.path.as_str()))
            .collect();
        assert_eq!(
            changes,
            [
                (Action::Add, "src/new.rs"),
                (Action::Modify, "src/main.rs"),
                (Action::Delete, "old.txt"),
                (Action::Modify, "link"),
            ]
        );
    }

    #[test]
    fn root_commits_and_broken_records() {
        let (commit, parents) = parse_commit("abc\x1f5\x1fAda\x1fa@x\x1f").unwrap();
        assert!(parents.is_empty() && commit.changes.is_empty());
        // Older records without the parents field still parse.
        assert!(parse_commit("abc\x1f5\x1fAda\x1fa@x").is_some());
        assert!(parse_commit("abc\x1fsoon\x1fAda\x1fa@x\x1f").is_none());
        assert!(parse_commit("abc\x1f5\x1fAda").is_none());
        assert!(parse_commit("").is_none());
    }

    #[test]
    fn histories_merge_by_time_keeping_their_order() {
        let at = |repo: usize, sha: &str, time: i64| Commit {
            repo,
            time,
            ..commit(sha)
        };
        // Repository 0's dates go back in time (a rebase, a wrong clock): its order stays.
        let merged = merge(vec![
            vec![at(0, "a1", 10), at(0, "a2", 5), at(0, "a3", 30)],
            vec![at(1, "b1", 7), at(1, "b2", 20)],
            vec![],
        ]);
        let shas: Vec<&str> = merged.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(shas, ["b1", "a1", "a2", "b2", "a3"]);
    }

    #[test]
    fn merges_link_only_forward() {
        let mut commits: Vec<Commit> = ["a", "m", "b", "c"].iter().map(|s| commit(s)).collect();
        commits[0].merge = Some("m".into());
        // A merge before its branch commit (author dates), or one filtered out, links nothing.
        commits[2].merge = Some("a".into());
        commits[3].merge = Some("gone".into());
        link_merges(&mut commits);
        let merged: Vec<Option<usize>> = commits.iter().map(|c| c.merged_by).collect();
        assert_eq!(merged, [Some(1), None, None, None]);
    }

    #[test]
    fn a_linear_history_has_no_side_branches() {
        let mut commits: Vec<Commit> = ["a", "b", "c"].iter().map(|s| commit(s)).collect();
        let parents = [vec![], vec!["a".to_string()], vec!["b".to_string()]];
        mark_side_branches(&mut commits, &parents);
        assert!(commits.iter().all(|c| c.merge.is_none()));
        mark_side_branches(&mut [], &[]);
    }

    #[test]
    fn repository_names_are_unique() {
        let names = repo_names(&[
            PathBuf::from("no/such/web"),
            PathBuf::from("other/web"),
            PathBuf::from("api"),
            PathBuf::from("third/web"),
        ]);
        assert_eq!(names, ["web", "web-2", "api", "web-3"]);
    }

    fn changed(path: &str) -> Change {
        Change {
            action: Action::Modify,
            path: path.to_string(),
        }
    }

    fn by(author: &str, email: &str, paths: &[&str]) -> Commit {
        Commit {
            author: author.to_string(),
            email: email.to_string(),
            changes: paths.iter().map(|p| changed(p)).collect(),
            ..commit(author)
        }
    }

    fn filtered(config: &Config, mut commits: Vec<Commit>) -> Vec<(String, Vec<String>)> {
        Filter::new(config).apply(&mut commits);
        commits
            .into_iter()
            .map(|c| (c.sha, c.changes.into_iter().map(|c| c.path).collect()))
            .collect()
    }

    #[test]
    fn filters_drop_paths_and_commits_left_empty() {
        let config = Config {
            include: vec!["src/".into(), "*.toml".into()],
            exclude: vec!["*.lock".into(), "src/gen/**".into()],
            ..Config::default()
        };
        let commits = vec![
            by(
                "a",
                "",
                &["src/main.rs", "Cargo.lock", "Cargo.toml", "README.md"],
            ),
            by("b", "", &["Cargo.lock"]),
            by("c", "", &["src/gen/x.rs"]),
            // A merge has no changes of its own: it stays.
            by("d", "", &[]),
        ];
        let kept = filtered(&config, commits);
        let src = |p: &[&str]| p.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        assert_eq!(
            kept,
            [
                ("a".to_string(), src(&["src/main.rs", "Cargo.toml"])),
                ("d".to_string(), src(&[])),
            ]
        );
    }

    #[test]
    fn excluded_users_match_names_and_emails_in_any_case() {
        let config = Config {
            exclude_user: vec!["Renovate*".into(), "*@CI.example.org".into()],
            ..Config::default()
        };
        let commits = vec![
            by(
                "renovate[bot]",
                "29139614+renovate[bot]@users.noreply.github.com",
                &["a"],
            ),
            by("Build", "build@ci.example.org", &["a"]),
            by("Ada", "ada@example.org", &["a"]),
            by("RENOVATE", "x@y", &[]),
        ];
        let kept: Vec<String> = filtered(&config, commits)
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(kept, ["Ada"]);
    }

    #[test]
    fn no_filters_keep_everything() {
        let commits = vec![by("a", "", &["x"]), by("b", "", &[])];
        assert_eq!(filtered(&Config::default(), commits).len(), 2);
    }

    const DAY: i64 = 86_400;
    /// 2024-01-01, 12:00 UTC: test commits are made on days after it.
    const NOON: i64 = 1_704_110_400;

    /// A throwaway git repository under the system's temp dir, removed when dropped.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Self {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!("eridu-git-{}-{n}", std::process::id()));
            let repo = Self(root.join(name));
            std::fs::create_dir_all(&repo.0).unwrap();
            repo.git(&["init", "-q", "-b", "main"], 0);
            repo
        }

        /// Runs git with nothing from the user's own config, dated `day` days after `NOON`.
        fn git(&self, args: &[&str], day: i64) -> String {
            let date = format!("{} +0000", NOON + day * DAY);
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", self.0.join("no-global-config"))
                .env("GIT_AUTHOR_DATE", &date)
                .env("GIT_COMMITTER_DATE", &date)
                .env("GIT_COMMITTER_NAME", "Committer")
                .env("GIT_COMMITTER_EMAIL", "committer@example.org")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        /// Writes (`Some`) or deletes (`None`) files, then commits them all as `author` on `day`.
        fn commit(&self, day: i64, author: &str, files: &[(&str, Option<&str>)]) -> String {
            for (path, text) in files {
                let file = self.0.join(path);
                match text {
                    Some(text) => {
                        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                        std::fs::write(&file, text).unwrap();
                    }
                    None => std::fs::remove_file(&file).unwrap(),
                }
            }
            self.git(&["add", "-A"], day);
            let who = format!("--author={author} <{}@Example.org>", author.to_lowercase());
            self.git(
                &["commit", "-q", "--allow-empty", "-m", "change", &who],
                day,
            );
            self.git(&["rev-parse", "HEAD"], day)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    /// Day 0: Ada adds a and b; day 1: Bob changes a; day 2: Ada deletes b; day 3: Bob adds c.
    fn four_days() -> Repo {
        let repo = Repo::new("four");
        repo.commit(0, "Ada", &[("a.txt", Some("1")), ("b.txt", Some("1"))]);
        repo.commit(1, "Bob", &[("a.txt", Some("2"))]);
        repo.commit(2, "Ada", &[("b.txt", None)]);
        repo.commit(3, "Bob", &[("docs/c.md", Some("1"))]);
        repo
    }

    fn changes(commit: &Commit) -> Vec<(Action, &str)> {
        commit
            .changes
            .iter()
            .map(|c| (c.action, c.path.as_str()))
            .collect()
    }

    fn range(from: Option<i64>, to: Option<i64>) -> Range {
        Range { from, to }
    }

    #[test]
    fn a_repository_is_read_oldest_first() {
        let repo = four_days();
        let commits = load(&repo.path()).unwrap();
        assert_eq!(commits.len(), 4);
        let times: Vec<i64> = commits.iter().map(|c| c.time).collect();
        assert_eq!(times, [NOON, NOON + DAY, NOON + 2 * DAY, NOON + 3 * DAY]);
        assert_eq!(commits[0].author, "Ada");
        assert_eq!(commits[0].email, "ada@example.org");
        assert_eq!(commits[0].sha.len(), 40);
        let mut first = changes(&commits[0]);
        first.sort_by_key(|c| c.1);
        assert_eq!(first, [(Action::Add, "a.txt"), (Action::Add, "b.txt")]);
        assert_eq!(changes(&commits[1]), [(Action::Modify, "a.txt")]);
        assert_eq!(changes(&commits[2]), [(Action::Delete, "b.txt")]);
        assert_eq!(changes(&commits[3]), [(Action::Add, "docs/c.md")]);
    }

    #[test]
    fn paths_are_not_quoted() {
        let repo = Repo::new("names");
        repo.commit(0, "Ada", &[("docs/été notes.md", Some("1"))]);
        let commits = load(&repo.path()).unwrap();
        assert_eq!(changes(&commits[0]), [(Action::Add, "docs/été notes.md")]);
    }

    #[test]
    fn renames_are_a_delete_and_an_add() {
        let repo = Repo::new("renames");
        repo.commit(
            0,
            "Ada",
            &[("old.txt", Some("same text, long enough to match"))],
        );
        repo.git(&["mv", "old.txt", "new.txt"], 1);
        repo.commit(1, "Ada", &[]);
        let commits = load(&repo.path()).unwrap();
        let mut renamed = changes(&commits[1]);
        renamed.sort_by_key(|c| c.1);
        assert_eq!(
            renamed,
            [(Action::Add, "new.txt"), (Action::Delete, "old.txt")]
        );
    }

    #[test]
    fn merges_delete_the_files_they_drop() {
        let repo = Repo::new("merges");
        repo.commit(0, "Ada", &[("a.txt", Some("1"))]);
        repo.git(&["checkout", "-q", "-b", "side"], 1);
        repo.commit(1, "Bob", &[("b.txt", Some("1"))]);
        repo.git(&["checkout", "-q", "main"], 2);
        repo.commit(2, "Ada", &[("a.txt", Some("2"))]);
        // Bob's branch is merged without his file: it must disappear at the merge.
        repo.git(&["merge", "-q", "--no-ff", "--no-commit", "side"], 3);
        repo.git(&["rm", "-q", "-f", "b.txt"], 3);
        repo.git(
            &[
                "commit",
                "-q",
                "-m",
                "merge",
                "--author=Ada <ada@example.org>",
            ],
            3,
        );
        let commits = load(&repo.path()).unwrap();
        assert_eq!(commits.len(), 4);
        assert_eq!(changes(&commits[3]), [(Action::Delete, "b.txt")]);
        let loaded = history(&[repo.path()], &Config::default(), range(None, None)).unwrap();
        let merged: Vec<Option<usize>> = loaded.commits.iter().map(|c| c.merged_by).collect();
        assert_eq!(merged, [None, Some(3), None, None]);
    }

    #[test]
    fn history_applies_filters() {
        let repo = four_days();
        let config = Config {
            exclude: vec!["docs/".into()],
            exclude_user: vec!["bob".into()],
            ..Config::default()
        };
        let loaded = history(&[repo.path()], &config, range(None, None)).unwrap();
        let authors: Vec<&str> = loaded.commits.iter().map(|c| c.author.as_str()).collect();
        assert_eq!(authors, ["Ada", "Ada"]);
        let config = Config {
            include: vec!["nothing/".into()],
            ..Config::default()
        };
        let error = history(&[repo.path()], &config, range(None, None))
            .err()
            .unwrap();
        assert!(error.starts_with("no commits to show"), "{error}");
    }

    #[test]
    fn history_starts_and_stops_where_asked() {
        let repo = four_days();
        let paths = [repo.path()];
        let load = |from, to, start| {
            let config = Config {
                start,
                ..Config::default()
            };
            history(&paths, &config, range(from, to))
        };
        let all = load(None, None, None).unwrap();
        assert_eq!((all.commits.len(), all.start), (4, 0));
        // `--from` keeps the earlier commits, to apply them at once.
        let from = load(Some(NOON + DAY), None, None).unwrap();
        assert_eq!((from.commits.len(), from.start), (4, 1));
        let to = load(None, Some(NOON + 2 * DAY), None).unwrap();
        assert_eq!(to.commits.len(), 3);
        // Halfway through days 0 to 3 is day 1.5: the first commit after it is day 2's.
        assert_eq!(load(None, None, Some(50.0)).unwrap().start, 2);
        // `--start` counts from `--from`: halfway through days 1 to 3 is day 2.
        assert_eq!(load(Some(NOON + DAY), None, Some(50.0)).unwrap().start, 2);
        assert_eq!(load(None, None, Some(99.0)).unwrap().start, 3);
        let error = load(Some(NOON + 4 * DAY), None, None).err().unwrap();
        assert_eq!(error, "no commits after --from");
    }

    #[test]
    fn range_takes_dates_tags_and_commits() {
        let repo = four_days();
        repo.git(&["tag", "v1", "HEAD~2"], 5);
        let sha = repo.git(&["rev-parse", "HEAD~1"], 5);
        let resolve = |from: &str, to: &str| {
            let config = Config {
                from: Some(from.to_string()),
                to: Some(to.to_string()),
                ..Config::default()
            };
            Range::new(&[repo.path()], &config).map(|r| (r.from, r.to))
        };
        let day = parse_date("2024-01-02").unwrap();
        // A date's whole day is included.
        assert_eq!(
            resolve("2024-01-02", "2024-01-02"),
            Ok((Some(day), Some(day + DAY - 1)))
        );
        assert_eq!(
            resolve("v1", &sha),
            Ok((Some(NOON + DAY), Some(NOON + 2 * DAY)))
        );
        let error = resolve("v9", "2024-01-02").unwrap_err();
        assert!(error.starts_with("`v9` is neither a date"), "{error}");
        let none = Range::new(&[repo.path()], &Config::default()).unwrap();
        assert_eq!((none.from, none.to), (None, None));
    }

    #[test]
    fn range_looks_in_every_repository() {
        let (a, b) = (four_days(), Repo::new("other"));
        b.commit(10, "Cy", &[("x", Some("1"))]);
        b.git(&["tag", "late"], 10);
        let config = Config {
            from: Some("late".to_string()),
            ..Config::default()
        };
        let range = Range::new(&[a.path(), b.path()], &config).unwrap();
        assert_eq!(range.from, Some(NOON + 10 * DAY));
    }

    #[test]
    fn tags_come_with_their_dates() {
        let repo = four_days();
        repo.git(&["tag", "v2", "HEAD"], 7);
        repo.git(&["tag", "v1", "HEAD~3"], 7);
        // An annotated tag is dated by its tagger.
        repo.git(&["tag", "-a", "v1.5", "-m", "release", "HEAD~2"], 8);
        assert_eq!(
            tags(&repo.path()),
            [
                (NOON, "v1".to_string()),
                (NOON + 3 * DAY, "v2".to_string()),
                (NOON + 8 * DAY, "v1.5".to_string()),
            ]
        );
        assert!(tags(Path::new("no/such/repository")).is_empty());
    }

    #[test]
    fn several_repositories_share_one_timeline() {
        let (a, b) = (Repo::new("alpha"), Repo::new("beta"));
        a.commit(0, "Ada", &[("x.txt", Some("1"))]);
        b.commit(1, "Bob", &[("src/y.rs", Some("1"))]);
        a.commit(2, "Ada", &[("x.txt", Some("2"))]);
        b.git(&["tag", "v1"], 1);
        let loaded = history(&[a.path(), b.path()], &Config::default(), range(None, None)).unwrap();
        let seen: Vec<(usize, &str)> = loaded
            .commits
            .iter()
            .map(|c| (c.repo, c.changes[0].path.as_str()))
            .collect();
        assert_eq!(
            seen,
            [(0, "alpha/x.txt"), (1, "beta/src/y.rs"), (0, "alpha/x.txt")]
        );
        assert_eq!(loaded.tags, [(NOON + DAY, "beta v1".to_string())]);
    }

    #[test]
    fn a_missing_repository_is_an_error() {
        let missing = std::env::temp_dir().join("eridu-git-no-such-repository");
        let error = history(&[missing], &Config::default(), range(None, None));
        assert!(error.is_err());
    }
}
