//! Who's who: groups commit authors into people and names them. People are shown by first
//! name only, never by last name, so a video of a repository keeps them anonymous.
//! GitHub lookups (logins, profile names, avatars) live in `github.rs`.

use std::collections::{BTreeMap, HashMap};

/// `[people]` of the config: a display name → the emails or commit names of that person.
pub type Aliases = BTreeMap<String, Vec<String>>;

/// Compact forms too generic to identify anyone (default git names, machine accounts).
const GENERIC: &[&str] = &[
    "yourname",
    "unknown",
    "localhost",
    "ubuntu",
    "github",
    "noreply",
    "gitlab",
    "jenkins",
    // Shared addresses (`contact@…`, `support@…`), not a person's name.
    "contact",
    "support",
    "developer",
    "notifications",
];

/// Words that mark an account as a machine (CI, infrastructure, bots), shown as written.
const MACHINE: &[&str] = &[
    "bot",
    "user",
    "terraform",
    "jenkins",
    "admin",
    "root",
    "build",
    "deploy",
    "automation",
    "ci",
    "stacks",
    "atlantis",
    "renovate",
    "renovatebot",
    "dependabot",
    "autofix",
];

/// Where a display name comes from, for `--people`.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Source {
    #[default]
    Email,
    Config,
    Name,
    Profile,
    Login,
    Machine,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Email => "email",
            Source::Config => "config",
            Source::Name => "name",
            Source::Profile => "profile",
            Source::Login => "login",
            Source::Machine => "machine",
        }
    }
}

/// One person, possibly behind several emails and spellings.
#[derive(Default)]
pub struct Person {
    pub emails: Vec<String>,
    /// Commit names as written, with how many commits used each.
    pub names: Vec<(String, usize)>,
    pub commits: usize,
    pub login: Option<String>,
    /// The name on their GitHub profile.
    pub profile: Option<String>,
    /// Their name in the config's `[people]`.
    pub alias: Option<String>,
    /// Set when they turned out to be the same person as another one.
    pub merged_into: Option<usize>,
    /// What's shown: a first name, never a last name (see `name`).
    pub display: String,
    pub source: Source,
}

/// Everyone in the history, numbered in order of first appearance.
#[derive(Default)]
pub struct People {
    pub list: Vec<Person>,
    pub by_email: HashMap<String, usize>,
    /// GitHub login → person, to merge people the email heuristics missed.
    logins: HashMap<String, usize>,
    /// `(from, into)` merges since the last `take_merges`.
    merges: Vec<(usize, usize)>,
    /// Commits per email, to tell a person's own emails from ones used in passing.
    email_commits: HashMap<String, usize>,
}

impl People {
    /// Groups the `(name, email)` of every commit into people, and names them.
    pub fn new<'a>(
        authors: impl Iterator<Item = (&'a str, &'a str)> + Clone,
        aliases: &Aliases,
    ) -> Self {
        let by_email = group_people(authors.clone(), aliases);
        let mut list: Vec<Person> = Vec::new();
        let mut email_commits: HashMap<String, usize> = HashMap::new();
        for (name, email) in authors {
            *email_commits.entry(email.to_string()).or_default() += 1;
            let p = by_email[email];
            if p == list.len() {
                list.push(Person::default());
            }
            let person = &mut list[p];
            person.commits += 1;
            if !person.emails.iter().any(|e| e == email) {
                person.emails.push(email.to_string());
            }
            match person.names.iter_mut().find(|(n, _)| n == name) {
                Some((_, count)) => *count += 1,
                None => person.names.push((name.to_string(), 1)),
            }
        }
        for person in &mut list {
            person.alias = aliases
                .iter()
                .find(|(_, entries)| entries.iter().any(|entry| person.answers_to(entry)))
                .map(|(alias, _)| alias.clone());
        }
        let mut people = People {
            list,
            by_email,
            email_commits,
            ..default()
        };
        people.name_all();
        people
    }

    /// Follows merges to the person who now stands for `p`.
    pub fn resolve(&self, mut p: usize) -> usize {
        while let Some(into) = self.list[p].merged_into {
            p = into;
        }
        p
    }

    /// Makes `into` stand for `from` from now on.
    fn merge(&mut self, from: usize, into: usize) {
        let from_person = std::mem::take(&mut self.list[from]);
        for email in &from_person.emails {
            self.by_email.insert(email.clone(), into);
        }
        let person = &mut self.list[into];
        person.emails.extend(from_person.emails);
        for (name, count) in from_person.names {
            match person.names.iter_mut().find(|(n, _)| *n == name) {
                Some((_, c)) => *c += count,
                None => person.names.push((name, count)),
            }
        }
        person.commits += from_person.commits;
        person.login = person.login.take().or(from_person.login);
        person.profile = person.profile.take().or(from_person.profile);
        person.alias = person.alias.take().or(from_person.alias);
        self.list[from].merged_into = Some(into);
        self.merges.push((from, into));
    }

    /// The merges done since the last call, as `(from, into)`.
    pub fn take_merges(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.merges)
    }

    /// Learns what GitHub knows about the person behind `email`, merging them with whoever has
    /// the same account or profile name. Returns the person, or `None` for an unknown email.
    pub fn learn(
        &mut self,
        email: &str,
        login: Option<&str>,
        profile: Option<&str>,
    ) -> Option<usize> {
        let mut p = self.resolve(*self.by_email.get(email)?);
        // An email used in passing (a few commits of someone with many) tells whose account it
        // is, not who this person is: four commits on a colleague's email merged them whole.
        let commits = self.email_commits.get(email).copied().unwrap_or(0);
        if commits * 10 < self.list[p].commits {
            return Some(p);
        }
        if let Some(login) = login {
            // GitHub logins ignore case: a noreply email spells `varetp` what the API calls `VaretP`.
            let key = login.to_lowercase();
            match self.logins.get(&key).map(|&q| self.resolve(q)) {
                // Same GitHub account as another person: credit their commits to the first one.
                Some(first) if first != p => {
                    self.merge(p, first);
                    p = first;
                }
                Some(_) => {}
                None => {
                    self.logins.insert(key, p);
                }
            }
            self.list[p].login.get_or_insert_with(|| login.to_string());
        }
        if let Some(profile) = profile.filter(|n| !n.trim().is_empty()) {
            self.list[p]
                .profile
                .get_or_insert_with(|| profile.to_string());
            // Someone else committing under the full name of this profile is this person too.
            let key = sorted_words(profile);
            if key.contains(' ') {
                let same: Vec<usize> = (0..self.list.len())
                    .filter(|&q| q != p && self.list[q].merged_into.is_none())
                    .filter(|&q| {
                        self.list[q]
                            .names
                            .iter()
                            .any(|(n, _)| sorted_words(n) == key)
                    })
                    .collect();
                if let [q] = same[..] {
                    let (from, into) = (p.max(q), p.min(q));
                    self.merge(from, into);
                    p = into;
                }
            }
        }
        Some(p)
    }

    /// Recomputes every display name, and returns the people whose name changed.
    pub fn name_all(&mut self) -> Vec<usize> {
        let mut named: Vec<(String, Source)> = self.list.iter().map(name).collect();
        // Two people shown by the same first name get the initial of their last name.
        let mut same: HashMap<String, Vec<usize>> = HashMap::new();
        for (p, (shown, source)) in named.iter().enumerate() {
            if self.list[p].merged_into.is_none()
                && matches!(source, Source::Name | Source::Profile)
            {
                same.entry(fold(shown)).or_default().push(p);
            }
        }
        for group in same.values().filter(|g| g.len() > 1) {
            for &p in group {
                if let Some(initial) = last_initial(&self.list[p]) {
                    named[p].0 = format!("{} {initial}.", named[p].0);
                }
            }
        }
        let mut changed = Vec::new();
        for (p, (shown, source)) in named.into_iter().enumerate() {
            let person = &mut self.list[p];
            if person.display != shown {
                person.display = shown;
                changed.push(p);
            }
            person.source = source;
        }
        changed
    }

    /// Everyone, most commits first: the name shown and where it comes from, commits, GitHub
    /// login, then their commit names, profile name and emails. For `--people`.
    pub fn report(&self) -> String {
        let mut shown: Vec<&Person> = self
            .list
            .iter()
            .filter(|p| p.merged_into.is_none())
            .collect();
        shown.sort_by_key(|p| std::cmp::Reverse(p.commits));
        let emails: usize = shown.iter().map(|p| p.emails.len()).sum();
        let mut out = format!("{} people behind {emails} emails\n", shown.len());
        for p in shown {
            let login = p
                .login
                .as_deref()
                .map_or(String::new(), |l| format!("  @{l}"));
            out += &format!(
                "\n{:<20} {:<8} {:>6} commits{login}\n",
                p.display,
                p.source.label(),
                p.commits
            );
            let mut names: Vec<&(String, usize)> = p.names.iter().collect();
            names.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            out += &format!(
                "    {}\n",
                names
                    .iter()
                    .map(|(n, _)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(profile) = &p.profile {
                out += &format!("    GitHub profile: {profile}\n");
            }
            out += &format!("    {}\n", p.emails.join(", "));
        }
        out
    }
}

impl Person {
    /// Whether an email or a commit name of theirs is `entry`, ignoring case.
    fn answers_to(&self, entry: &str) -> bool {
        let entry = entry.trim().to_lowercase();
        self.emails.iter().any(|e| e.to_lowercase() == entry)
            || self.names.iter().any(|(n, _)| n.to_lowercase() == entry)
    }
}

fn default<T: Default>() -> T {
    T::default()
}

/// Groups emails into people. Git histories are full of one person committing under several
/// emails and spellings, so two emails are linked whenever they share a clue (see `clues`),
/// or when one's local part spells a person's initials (see `link_initials`).
/// Returns each email's person, numbered in order of first appearance.
///
/// `authors` has one entry per commit. A name seen on someone else's email only in passing
/// (a co-authored or mis-attributed commit: under half of that email's commits and under half
/// of that name's) is no clue, or one stray commit would merge two people whole.
pub fn group_people<'a>(
    authors: impl IntoIterator<Item = (&'a str, &'a str)>,
    aliases: &Aliases,
) -> HashMap<String, usize> {
    let authors: Vec<(&str, &str)> = authors.into_iter().collect();
    let mut pairs: HashMap<(&str, &str), usize> = HashMap::new();
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    let mut by_email: HashMap<&str, usize> = HashMap::new();
    for &(name, email) in &authors {
        *pairs.entry((name, email)).or_default() += 1;
        *by_name.entry(name).or_default() += 1;
        *by_email.entry(email).or_default() += 1;
    }
    let stray = |name: &str, email: &str| {
        let pair = pairs[&(name, email)];
        pair * 2 < by_email[email] && pair * 2 < by_name[name]
    };
    let mut emails: Vec<&str> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    let mut owners: HashMap<String, usize> = HashMap::new();
    let mut named: Vec<(usize, &str)> = Vec::new();
    // Each alias entry, lowercased, to the alias it belongs to.
    let alias_of: HashMap<String, &str> = aliases
        .iter()
        .flat_map(|(alias, entries)| {
            entries
                .iter()
                .map(move |e| (e.trim().to_lowercase(), alias.as_str()))
        })
        .collect();
    for (name, email) in authors {
        let e = *index.entry(email).or_insert_with(|| {
            emails.push(email);
            parent.push(parent.len());
            parent.len() - 1
        });
        // A stray commit's name says nothing about the email.
        let name = if stray(name, email) { "" } else { name };
        if !name.is_empty() && !named.contains(&(e, name)) {
            named.push((e, name));
        }
        let alias = alias_of.get(&email.to_lowercase()).or_else(|| {
            alias_of
                .get(&name.to_lowercase())
                .filter(|_| !name.is_empty())
        });
        for clue in clues(name, email)
            .into_iter()
            .chain(alias.map(|a| format!("alias:{a}")))
        {
            match owners.get(&clue) {
                Some(&other) => union(&mut parent, e, other),
                None => {
                    owners.insert(clue, e);
                }
            }
        }
    }
    link_initials(&emails, &named, &mut parent);
    link_unset_emails(&emails, &named, &mut parent);
    let mut people: HashMap<usize, usize> = HashMap::new();
    (0..emails.len())
        .map(|e| {
            let root = find(&mut parent, e);
            let count = people.len();
            (emails[e].to_string(), *people.entry(root).or_insert(count))
        })
        .collect()
}

/// What can tie an email to a person: the email itself, the author name, the "first.last" part
/// of the email, and the GitHub account of a noreply address. Names and email parts are compared
/// as words (`Jérôme DUPONT` = `dupont jerome`) and squashed (`Ada Lovelace` = `adalovelace@…`).
fn clues(name: &str, email: &str) -> Vec<String> {
    let mut clues = vec![format!("email:{email}")];
    word_clues(name, &mut clues);
    match noreply_account(email) {
        Some((id, login)) => {
            clues.extend(id.map(|id| format!("github:{id}")));
            word_clues(&login, &mut clues);
        }
        None => word_clues(&local_words(email).join("."), &mut clues),
    }
    clues
}

fn word_clues(text: &str, clues: &mut Vec<String>) {
    let mut words = folded_words(text);
    let squashed = words.concat();
    if squashed.len() < 6 || GENERIC.contains(&squashed.as_str()) {
        return;
    }
    clues.push(format!("squashed:{squashed}"));
    if words.len() >= 2 {
        words.sort_unstable();
        clues.push(format!("words:{}", words.join(" ")));
    }
}

/// Links an email whose local part is a person's initial + last name or first name + initial
/// (`jdoe@…`, `johnd@…`, `jpsmith@…` for Jean-Pierre Smith), or starts with their full name
/// (`johndoe1@…`). Only an email is linked to a name, never two names, and only when a single
/// person matches, so John and Jane Doe don't become one `jdoe`.
fn link_initials(emails: &[&str], named: &[(usize, &str)], parent: &mut [usize]) {
    let mut forms: HashMap<String, Vec<usize>> = HashMap::new();
    let mut full: Vec<(String, usize)> = Vec::new();
    for &(e, name) in named {
        let words: Vec<String> = name_words(name)
            .iter()
            .map(|w| fold(w))
            .filter(|w| !w.is_empty())
            .collect();
        if words.len() < 2 || GENERIC.contains(&words.concat().as_str()) {
            continue;
        }
        let initials = |word: &str| -> String {
            let original = name_words(name)
                .into_iter()
                .find(|w| fold(w) == word)
                .unwrap_or(word);
            original
                .split('-')
                .filter_map(|part| part.chars().next())
                .map(fold_char)
                .collect()
        };
        let (first, rest) = (&words[0], words[1..].concat());
        let head = |word: &str| word.chars().next().map(String::from).unwrap_or_default();
        let mut add = |form: String| forms.entry(form).or_default().push(e);
        add(format!("{}{rest}", initials(first)));
        add(format!("{first}{}", head(&rest)));
        if words.len() == 2 {
            // `TURING Alan`: the order is unknown, so try both.
            add(format!("{}{first}", initials(&words[1])));
            add(format!("{}{}", words[1], head(first)));
        }
        full.push((words.concat(), e));
    }
    for (e, email) in emails.iter().enumerate() {
        if noreply_account(email).is_some() {
            continue;
        }
        let local = local_words(email).concat();
        if local.len() < 5 || GENERIC.contains(&local.as_str()) {
            continue;
        }
        let prefixed = full.iter().filter(|(f, _)| {
            f.len() >= 7 && local.len() > f.len() && local.starts_with(f.as_str())
        });
        let mut roots: Vec<usize> = forms
            .get(&local)
            .into_iter()
            .flatten()
            .copied()
            .chain(prefixed.map(|&(_, n)| n))
            .collect();
        for root in &mut roots {
            *root = find(parent, *root);
        }
        roots.sort_unstable();
        roots.dedup();
        if let [root] = roots[..] {
            union(parent, e, root);
        }
    }
}

/// Links the commits of a machine with no email set up (`ada@MacBook-Air-de-Ada.local`) to the
/// one other person with the same first name, if there is exactly one: such an email says nothing
/// about who it is, the name is all there is.
fn link_unset_emails(emails: &[&str], named: &[(usize, &str)], parent: &mut [usize]) {
    let unset = |email: &str| {
        let domain = email.rsplit_once('@').map_or("", |(_, d)| d);
        !domain.contains('.')
            || [".local", ".lan", ".home", ".localdomain"]
                .iter()
                .any(|tld| domain.ends_with(tld))
    };
    let first = |name: &str| first_word(name).map(|(first, _)| fold(first));
    let mut by_first: HashMap<String, Vec<usize>> = HashMap::new();
    for &(e, name) in named.iter().filter(|(e, _)| !unset(emails[*e])) {
        if let Some(first) = first(name) {
            by_first.entry(first).or_default().push(e);
        }
    }
    for &(e, name) in named.iter().filter(|(e, _)| unset(emails[*e])) {
        let Some(others) = first(name).and_then(|first| by_first.get(&first)) else {
            continue;
        };
        let mut roots: Vec<usize> = others.iter().map(|&o| find(parent, o)).collect();
        roots.sort_unstable();
        roots.dedup();
        if let [root] = roots[..] {
            union(parent, e, root);
        }
    }
}

/// A person's display name: their first name, else their GitHub login, else their email's local
/// part, each without their last name. A `[people]` alias wins; machines keep their name.
fn name(p: &Person) -> (String, Source) {
    if let Some(alias) = &p.alias {
        return (alias.clone(), Source::Config);
    }
    if !p.names.is_empty() && p.names.iter().all(|(n, _)| is_machine(n)) {
        let (n, _) = p.names.iter().max_by_key(|(_, c)| *c).expect("not empty");
        return (n.trim_end_matches("[bot]").to_string(), Source::Machine);
    }
    let last = last_names(p);
    if let Some(first) = first_name(p, &last) {
        return first;
    }
    let login = p
        .login
        .clone()
        .or_else(|| p.emails.iter().find_map(|e| noreply_login(e)));
    if let Some(login) = &login
        && let Some(curated) = curated_handle(login, &last)
    {
        return (curated, Source::Login);
    }
    if let Some(curated) = p
        .emails
        .iter()
        .filter(|e| noreply_account(e).is_none())
        .find_map(|e| curated_email(e, &last))
    {
        return (curated, Source::Email);
    }
    // Nothing hides the last name: the login or email as it is.
    let handle = login.or_else(|| p.emails.first().map(|e| local_part(e).to_string()));
    (handle.unwrap_or_else(|| "?".to_string()), Source::Email)
}

/// The best first name among the person's commit names and GitHub profile name.
/// Names of two words or more win over single words (often a username: `jdoe`, `AnnaKb7`),
/// then the most used. Accents and capitals pick the spelling (`Jérémy` over `jeremy`).
fn first_name(p: &Person, last: &[String]) -> Option<(String, Source)> {
    let logins = p
        .login
        .iter()
        .cloned()
        .chain(p.emails.iter().filter_map(|e| noreply_login(e)));
    let handles: Vec<String> = p
        .emails
        .iter()
        .map(|e| local_words(e).concat())
        .chain(logins.map(|l| fold(&l)))
        .collect();
    let profile = p.profile.iter().map(|n| (n, 1, Source::Profile));
    let names = p.names.iter().map(|(n, c)| (n, *c, Source::Name));
    // (spelling, folded, several words, commits, source)
    let mut candidates: Vec<(&str, String, bool, usize, Source)> = Vec::new();
    for (name, count, source) in names.chain(profile) {
        let Some((first, several)) = first_word(name) else {
            continue;
        };
        let folded = fold(first);
        // `Hugo` with `hugo@…` is a name; `jdoe` with `jdoe@…` is a username.
        let capitalized = first.chars().next().is_some_and(char::is_uppercase)
            && !first.chars().skip(1).any(char::is_uppercase);
        let looks_like_handle = !several
            && (last.contains(&folded)
                || (handles.contains(&folded) && !capitalized)
                || last.iter().any(|l| {
                    l.len() >= 3
                        && folded.len() > l.len()
                        && (folded.starts_with(l.as_str()) || folded.ends_with(l.as_str()))
                }));
        if folded.len() >= 2 && !looks_like_handle {
            candidates.push((first, folded, several, count, source));
        }
    }
    // Votes per first name: does any spelling come with a last name, and how many commits.
    let mut votes: HashMap<&str, (bool, usize)> = HashMap::new();
    for (_, folded, several, count, _) in &candidates {
        let vote = votes.entry(folded).or_default();
        vote.0 |= *several;
        vote.1 += count;
    }
    let (winner, _) = votes
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))?;
    let winner = winner.to_string();
    let spellings = candidates.iter().filter(|c| c.1 == winner);
    let source = if spellings.clone().any(|c| c.4 == Source::Name) {
        Source::Name
    } else {
        Source::Profile
    };
    let (spelling, ..) = spellings.max_by_key(|(s, _, _, count, _)| {
        let accented = !s.is_ascii();
        let capitalized =
            s.chars().next().is_some_and(char::is_uppercase) && s.chars().any(char::is_lowercase);
        (accented, capitalized, *count)
    })?;
    Some((tidy_case(spelling), source))
}

/// The first name in a written name, and whether the name has more words (a last name).
/// In `Grace DE HOPPER` or `TURING Alan`, words in capitals are last names; initials (`Paolo R.`)
/// don't count. A single word must look like a name, not a username (no digits, no `CamelCase`,
/// no lowercase `first-word`).
fn first_word(name: &str) -> Option<(&str, bool)> {
    if is_machine(name) || GENERIC.contains(&fold(name).as_str()) {
        return None;
    }
    let words = name_words(name);
    let real: Vec<&str> = words
        .iter()
        .copied()
        .filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 2)
        .collect();
    match real[..] {
        [] => None,
        [word] if words.len() == 1 && !is_handle(word) => Some((word, false)),
        // `AnnaKb7`, `TomG`, `Ada1815`: a username that starts with a first name.
        [word] if words.len() == 1 => leading_name(word).map(|first| (first, false)),
        _ => Some((
            real.iter()
                .copied()
                .find(|w| !is_upper(w))
                .unwrap_or(real[0]),
            true,
        )),
    }
}

/// Words usernames start with that are no first name (`TheRawMeatball`, `BigCheese`), folded.
const NOT_FIRST_NAMES: &[&str] = &[
    "the", "big", "little", "lil", "real", "not", "just", "its", "your", "our", "old", "new",
    "one", "cool", "dark", "evil", "mad", "super", "captain", "happy", "lord", "sir", "mister",
];

/// The capitalized word a username starts with (`AnnaKb7` → `Anna`), if it has 3 letters or more
/// and isn't a word like `The` (`TheRawMeatball`).
fn leading_name(word: &str) -> Option<&str> {
    let mut chars = word.char_indices();
    chars.next().filter(|(_, c)| c.is_uppercase())?;
    let end = chars
        .find(|&(_, c)| !(c.is_lowercase() || is_mark(c)))
        .map_or(word.len(), |(i, _)| i);
    let name = &word[..end];
    (end < word.len()
        && name.chars().filter(|c| c.is_alphabetic()).count() >= 3
        && !NOT_FIRST_NAMES.contains(&fold(name).as_str()))
    .then_some(name)
}

/// Last names the person goes by, folded: the other words of their names, and what follows the
/// first part of `first.last` emails.
fn last_names(p: &Person) -> Vec<String> {
    let mut last: Vec<String> = Vec::new();
    for name in p.names.iter().map(|(n, _)| n).chain(&p.profile) {
        let Some((first, true)) = first_word(name) else {
            continue;
        };
        for word in name_words(name).into_iter().filter(|w| *w != first) {
            let folded = fold(word);
            if folded.len() >= 2 && !last.contains(&folded) {
                last.push(folded);
            }
        }
    }
    for email in p.emails.iter().filter(|e| noreply_account(e).is_none()) {
        let words = local_words(email);
        if words.len() >= 2 && words.iter().all(|w| w.len() >= 2) {
            for word in &words[1..] {
                if !last.contains(word) {
                    last.push(word.clone());
                }
            }
        }
    }
    last
}

/// The initial of the person's most used last name, to tell two people of the same first name apart.
fn last_initial(p: &Person) -> Option<char> {
    let (name, _) = p
        .names
        .iter()
        .filter(|(n, _)| matches!(first_word(n), Some((_, true))))
        .max_by_key(|(_, c)| *c)?;
    let (first, _) = first_word(name)?;
    let last = name_words(name)
        .into_iter()
        .find(|w| *w != first && w.chars().filter(|c| c.is_alphabetic()).count() >= 2)?;
    last.chars()
        .next()
        .map(|c| c.to_uppercase().next().unwrap_or(c))
}

/// A login or a squashed local part without the person's last name: `jane-doe` → `Jane`,
/// `janedoe` → `Jane`. `None` when the last name can't be taken out (`jdoe`).
fn curated_handle(handle: &str, last: &[String]) -> Option<String> {
    let parts: Vec<&str> = handle
        .split(['-', '_', '.'])
        .filter(|p| !p.is_empty())
        .collect();
    let kept: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|p| !last.contains(&fold(p)))
        .collect();
    if kept.is_empty() {
        return None;
    }
    if kept.len() < parts.len() {
        return Some(tidy_case(&kept.join("-")));
    }
    let folded = fold(handle);
    let Some(l) = last
        .iter()
        .find(|l| l.len() >= 3 && folded.contains(l.as_str()))
    else {
        return Some(handle.to_string());
    };
    match folded
        .strip_prefix(l.as_str())
        .or_else(|| folded.strip_suffix(l.as_str()))
    {
        Some(rest) if rest.len() >= 2 => Some(tidy_case(rest)),
        _ => None,
    }
}

/// An email's local part as a name: the first part of `first.last` (`jane.doe` → `Jane`),
/// else the local part without the last name (see `curated_handle`).
fn curated_email(email: &str, last: &[String]) -> Option<String> {
    let words = local_words(email);
    if words.len() >= 2
        && words
            .iter()
            .all(|w| w.len() >= 2 && w.chars().all(char::is_alphabetic))
    {
        return words
            .iter()
            .find(|w| !last.contains(*w))
            .map(|w| tidy_case(w));
    }
    curated_handle(local_part(email), last).filter(|c| c.len() >= 2)
}

/// Words of a written name: split on spaces, `_` and `.`, keeping `Jean-Philippe` whole.
fn name_words(name: &str) -> Vec<&str> {
    name.split(|c: char| c.is_whitespace() || c == '_' || c == '.')
        .filter(|w| !w.is_empty())
        .collect()
}

/// A word in capitals (`DOE`), a last name in many conventions.
fn is_upper(word: &str) -> bool {
    word.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !word.chars().any(char::is_lowercase)
}

/// A single word that reads as a username: digits (`Ada1815`), a capital after a lowercase
/// letter (`TuringA`, `TomG`), or lowercase hyphenated parts (`john-acme`). `Jean-Pierre` is a name.
fn is_handle(word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    let camel = chars
        .windows(2)
        .any(|w| w[0].is_lowercase() && w[1].is_uppercase());
    let hyphenated = word.contains('-')
        && !word
            .split('-')
            .all(|p| p.chars().next().is_some_and(char::is_uppercase));
    word.chars().any(|c| c.is_ascii_digit())
        || camel
        || hyphenated
        || !word
            .chars()
            .all(|c| c.is_alphabetic() || c == '-' || is_mark(c))
}

/// A bot, CI or infrastructure account.
fn is_machine(name: &str) -> bool {
    name.ends_with("[bot]")
        || folded_words(name)
            .iter()
            .any(|w| MACHINE.contains(&w.as_str()))
}

/// `jane` → `Jane`, `JEAN-PIERRE` → `Jean-Pierre`; mixed case is kept as written (`McKay`).
fn tidy_case(word: &str) -> String {
    if word.chars().any(char::is_lowercase) && word.chars().any(char::is_uppercase) {
        return word.to_string();
    }
    let capitalize = |part: &str| {
        let mut chars = part.chars();
        chars.next().map_or(String::new(), |c| {
            c.to_uppercase()
                .chain(chars.flat_map(char::to_lowercase))
                .collect()
        })
    };
    word.split('-')
        .map(capitalize)
        .collect::<Vec<_>>()
        .join("-")
}

/// The words of a name, sorted, to compare names whatever their order.
fn sorted_words(name: &str) -> String {
    let mut words = folded_words(name);
    words.sort_unstable();
    words.join(" ")
}

/// Lowercased, accent-free alphanumeric words.
fn folded_words(text: &str) -> Vec<String> {
    // Decomposed accents (`e` + U+0301, as macOS writes names) are dropped like composed ones.
    let folded: String = text
        .chars()
        .filter(|&c| !is_mark(c))
        .flat_map(char::to_lowercase)
        .map(fold_char)
        .collect();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Lowercased and accent-free, letters and digits only: `Jean-Philippe` → `jeanphilippe`.
fn fold(text: &str) -> String {
    folded_words(text).concat()
}

/// A combining accent.
fn is_mark(c: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&c)
}

fn fold_char(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'ø' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ÿ' | 'ý' => 'y',
        c => c,
    }
}

/// The part of an email before `@` and any `+tag`.
fn local_part(email: &str) -> &str {
    let local = email.split('@').next().unwrap_or(email);
    local.split('+').next().unwrap_or(local)
}

/// The words of an email's local part, without trailing digits: `jane.doe2@…` → `jane`, `doe`.
fn local_words(email: &str) -> Vec<String> {
    folded_words(local_part(email))
        .into_iter()
        .map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parent, a), find(parent, b));
    // The smaller index wins, so a person keeps their earliest email as root.
    parent[a.max(b)] = a.min(b);
}

pub fn noreply_login(email: &str) -> Option<String> {
    noreply_account(email).map(|(_, login)| login)
}

/// `12345+login@users.noreply.github.com` → (`Some("12345")`, `login`); older addresses have no id.
pub fn noreply_account(email: &str) -> Option<(Option<&str>, String)> {
    let local = email.strip_suffix("@users.noreply.github.com")?;
    Some(match local.split_once('+') {
        Some((id, login)) => (Some(id), login.to_string()),
        None => (None, local.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people(authors: &[(&str, &str)]) -> People {
        People::new(authors.iter().map(|&(n, e)| (n, e)), &Aliases::new())
    }

    /// The display name of the person behind `email`.
    fn shown(people: &People, email: &str) -> String {
        people.list[people.resolve(people.by_email[email])]
            .display
            .clone()
    }

    #[test]
    fn groups_the_emails_and_spellings_of_one_person() {
        let people = group_people(
            [
                ("Alan Turing", "alanturing@gmail.com"),
                ("alan turing", "alan.turing@acme.eu"),
                ("Ada LOVELACE", "ada.lovelace@acme.com"),
                ("ada lovelace", "lovelace.ada2@gmail.com"),
                ("grace", "grace.hopper@gmail.com"),
                ("Grace HOPPER", "ghopper@acme.com"),
                ("Grace Hopper", "ghopper@acme.com"),
                ("Linus", "linus.torvalds@acme.com"),
                ("Linus Torvalds", "linus@acme.com"),
                ("lt", "ltorvalds@other.org"),
                ("Ken T", "144787577+ken-acme@users.noreply.github.com"),
                ("ken", "144787577+ken-thompson@users.noreply.github.com"),
                ("Your Name", "ugo@example.com"),
                ("your name", "someone.else@example.com"),
                ("Margaret Hamilton", "mh@acme.com"),
                ("Maggie", "margarethamiltonx@gmail.com"),
            ],
            &Aliases::new(),
        );
        let same = |a: &str, b: &str| people[a] == people[b];
        assert!(same("alanturing@gmail.com", "alan.turing@acme.eu"));
        assert!(same("ada.lovelace@acme.com", "lovelace.ada2@gmail.com"));
        assert!(same("grace.hopper@gmail.com", "ghopper@acme.com"));
        // Initial + last name, and a local part starting with the full name.
        assert!(same("linus@acme.com", "ltorvalds@other.org"));
        assert!(same("mh@acme.com", "margarethamiltonx@gmail.com"));
        assert!(same(
            "144787577+ken-acme@users.noreply.github.com",
            "144787577+ken-thompson@users.noreply.github.com"
        ));
        assert!(!same("ugo@example.com", "someone.else@example.com"));
        assert!(!same("alanturing@gmail.com", "ada.lovelace@acme.com"));
        // People are numbered in order of appearance.
        assert_eq!(people["alanturing@gmail.com"], 0);
        assert_eq!(people["ada.lovelace@acme.com"], 1);
    }

    #[test]
    fn initials_never_merge_two_people() {
        let people = group_people(
            [
                ("John Doe", "john@acme.com"),
                ("Jane Doe", "jane@acme.com"),
                ("jdoe", "jdoe@acme.com"),
            ],
            &Aliases::new(),
        );
        assert_ne!(people["john@acme.com"], people["jane@acme.com"]);
        assert_ne!(people["jdoe@acme.com"], people["john@acme.com"]);
    }

    #[test]
    fn laptop_emails_go_by_first_name() {
        let people = group_people(
            [
                ("Ada Lovelace", "ada.lovelace@acme.com"),
                ("Ada", "ada@MacBook-Air-de-Ada.local"),
                ("Tom Martin", "tom@acme.com"),
                ("Tom Durand", "td@acme.com"),
                ("tom", "tom@laptop"),
            ],
            &Aliases::new(),
        );
        assert_eq!(
            people["ada@MacBook-Air-de-Ada.local"],
            people["ada.lovelace@acme.com"]
        );
        // Two Toms: no telling which one.
        assert_ne!(people["tom@laptop"], people["tom@acme.com"]);
        assert_ne!(people["tom@laptop"], people["td@acme.com"]);
    }

    #[test]
    fn shows_first_names_only() {
        let people = people(&[
            ("Ada LOVELACE", "ada@acme.com"),
            ("TURING Alan", "alan@acme.com"),
            ("Alan Turing", "alan@acme.com"),
            ("Grace DE HOPPER", "grace@acme.com"),
            ("jean-pierre noël", "jp@acme.com"),
            ("Jérémy Dupont", "jdupont@acme.com"),
            ("jeremy dupont", "jdupont@acme.com"),
            ("jeremy dupont", "jdupont@acme.com"),
            ("Paolo R.", "paolo@acme.com"),
            ("margaret", "mh@acme.com"),
            ("jdoe", "jdoe@acme.com"),
            ("TomG", "17992185+sg@users.noreply.github.com"),
            ("AnnaKb7", "elb@acme.com"),
            ("Hugo", "hugo@laptop.local"),
            ("Se\u{301}bastien", "seb@acme.com"),
            (
                "acme-renovatebot[bot]",
                "1+renovate[bot]@users.noreply.github.com",
            ),
            ("Terraform User", "itops+terraform@acme.com"),
            ("Your Name", "linus.torvalds@acme.com"),
        ]);
        assert_eq!(shown(&people, "ada@acme.com"), "Ada");
        assert_eq!(shown(&people, "alan@acme.com"), "Alan");
        assert_eq!(shown(&people, "grace@acme.com"), "Grace");
        assert_eq!(shown(&people, "jp@acme.com"), "Jean-Pierre");
        assert_eq!(shown(&people, "jdupont@acme.com"), "Jérémy");
        assert_eq!(shown(&people, "paolo@acme.com"), "Paolo");
        assert_eq!(shown(&people, "mh@acme.com"), "Margaret");
        // A username isn't a name: the email's local part, as it is when it can't be curated.
        assert_eq!(shown(&people, "jdoe@acme.com"), "jdoe");
        assert_eq!(
            shown(&people, "17992185+sg@users.noreply.github.com"),
            "Tom"
        );
        assert_eq!(shown(&people, "elb@acme.com"), "Anna");
        assert_eq!(shown(&people, "hugo@laptop.local"), "Hugo");
        assert_eq!(shown(&people, "seb@acme.com"), "Se\u{301}bastien");
        assert_eq!(
            shown(&people, "1+renovate[bot]@users.noreply.github.com"),
            "acme-renovatebot"
        );
        assert_eq!(shown(&people, "itops+terraform@acme.com"), "Terraform User");
        // `first.last` emails give a first name.
        assert_eq!(shown(&people, "linus.torvalds@acme.com"), "Linus");
    }

    #[test]
    fn github_fills_in_and_merges() {
        let mut people = people(&[
            ("AnnaKb7", "e@acme.com"),
            ("Anna Lovelace", "anna.l@home.org"),
            ("Alan Turing", "alan@acme.com"),
            ("at", "at@home.org"),
            ("kt", "kt@acme.com"),
        ]);
        assert_ne!(
            people.by_email["e@acme.com"],
            people.by_email["anna.l@home.org"]
        );
        // The profile name ties the username to the full name.
        people.learn("e@acme.com", Some("annakb7"), Some("Anna Lovelace"));
        people.learn("kt@acme.com", Some("kthompson"), Some("Ken Thompson"));
        // Same GitHub account.
        people.learn("alan@acme.com", Some("aturing"), None);
        people.learn("at@home.org", Some("aturing"), None);
        assert_eq!(people.take_merges().len(), 2);
        people.name_all();
        assert_eq!(shown(&people, "e@acme.com"), "Anna");
        assert_eq!(
            people.resolve(people.by_email["e@acme.com"]),
            people.resolve(people.by_email["anna.l@home.org"])
        );
        assert_eq!(shown(&people, "kt@acme.com"), "Ken");
        assert_eq!(shown(&people, "at@home.org"), "Alan");
    }

    #[test]
    fn logins_lose_the_last_name() {
        let mut people = people(&[("x", "x@acme.com"), ("y", "jane.doe@acme.com")]);
        people.learn("x@acme.com", Some("rocket-dev"), None);
        people.learn("jane.doe@acme.com", Some("jane-doe"), None);
        people.name_all();
        assert_eq!(shown(&people, "x@acme.com"), "rocket-dev");
        assert_eq!(shown(&people, "jane.doe@acme.com"), "Jane");
    }

    #[test]
    fn same_first_names_get_an_initial() {
        let people = people(&[
            ("Thomas Martin", "tm@acme.com"),
            ("Thomas Durand", "td@acme.com"),
            ("Ada Lovelace", "ada@acme.com"),
        ]);
        assert_eq!(shown(&people, "tm@acme.com"), "Thomas M.");
        assert_eq!(shown(&people, "td@acme.com"), "Thomas D.");
        assert_eq!(shown(&people, "ada@acme.com"), "Ada");
    }

    #[test]
    fn aliases_merge_and_name() {
        let aliases = Aliases::from([(
            "Gus".to_string(),
            vec!["gus@home.org".to_string(), "Ágúst Helgason".to_string()],
        )]);
        let people = People::new(
            [
                ("Ágúst Helgason", "ahelgason@acme.com"),
                ("Ágúst", "gus@home.org"),
            ]
            .into_iter(),
            &aliases,
        );
        assert_eq!(
            people.by_email["ahelgason@acme.com"],
            people.by_email["gus@home.org"]
        );
        assert_eq!(people.list[0].display, "Gus");
    }

    fn grouped(authors: &[(&str, &str)]) -> HashMap<String, usize> {
        group_people(authors.iter().copied(), &Aliases::new())
    }

    fn aliases(entries: &[(&str, &[&str])]) -> Aliases {
        entries
            .iter()
            .map(|(alias, of)| {
                (
                    alias.to_string(),
                    of.iter().map(|e| e.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn noreply_emails_give_the_github_account() {
        assert_eq!(
            noreply_account("1815+ada@users.noreply.github.com"),
            Some((Some("1815"), "ada".to_string()))
        );
        assert_eq!(
            noreply_account("grace@users.noreply.github.com"),
            Some((None, "grace".to_string()))
        );
        assert_eq!(noreply_account("ada+git@acme.com"), None);
        assert_eq!(noreply_account("ada@github.com"), None);
        assert_eq!(
            noreply_login("1815+ada@users.noreply.github.com").as_deref(),
            Some("ada")
        );
    }

    #[test]
    fn noreply_logins_and_tagged_emails_are_clues() {
        let people = grouped(&[
            ("Ada Lovelace", "ada@acme.com"),
            ("ada", "1815+adalovelace@users.noreply.github.com"),
            ("Hopper Grace", "grace@navy.mil"),
            ("g", "grace.hopper+git@acme.com"),
        ]);
        assert_eq!(
            people["ada@acme.com"],
            people["1815+adalovelace@users.noreply.github.com"]
        );
        // The `+tag` is dropped, and the word order doesn't matter.
        assert_eq!(
            people["grace@navy.mil"],
            people["grace.hopper+git@acme.com"]
        );
        assert_ne!(people["ada@acme.com"], people["grace@navy.mil"]);
    }

    #[test]
    fn accents_and_capitals_spell_the_same_name() {
        let people = grouped(&[
            ("Jérôme Dupont", "jerome@acme.com"),
            // Decomposed accents, as macOS writes them.
            ("Je\u{301}ro\u{302}me Dupont", "jd@laptop.org"),
            ("DUPONT JEROME", "dupont@other.org"),
        ]);
        assert_eq!(people["jerome@acme.com"], people["jd@laptop.org"]);
        assert_eq!(people["jerome@acme.com"], people["dupont@other.org"]);
    }

    #[test]
    fn short_or_generic_names_link_nobody() {
        let people = grouped(&[
            ("Al", "al@acme.com"),
            ("al", "al@other.org"),
            ("github", "a@acme.com"),
            ("GitHub", "b@acme.com"),
            ("unknown", "c@acme.com"),
            ("Unknown", "d@acme.com"),
        ]);
        assert_ne!(people["al@acme.com"], people["al@other.org"]);
        assert_ne!(people["a@acme.com"], people["b@acme.com"]);
        assert_ne!(people["c@acme.com"], people["d@acme.com"]);
    }

    #[test]
    fn initials_link_in_every_usual_form() {
        let people = grouped(&[
            ("Grace Hopper", "grace@navy.mil"),
            ("g", "graceh@acme.com"),
            ("Jean-Pierre Smith", "jean-pierre@acme.com"),
            ("jp", "jpsmith@acme.com"),
            // Order unknown with two words: the capitals may be first.
            ("TURING Alan", "alan@acme.com"),
            ("a", "aturing@other.org"),
        ]);
        assert_eq!(people["grace@navy.mil"], people["graceh@acme.com"]);
        assert_eq!(people["jean-pierre@acme.com"], people["jpsmith@acme.com"]);
        assert_eq!(people["alan@acme.com"], people["aturing@other.org"]);
        assert_ne!(people["grace@navy.mil"], people["alan@acme.com"]);
    }

    #[test]
    fn unset_emails_need_a_known_domain_or_none() {
        let people = grouped(&[
            ("Ada Lovelace", "ada.lovelace@acme.com"),
            ("Ada", "ada@raspberry.lan"),
            ("Ada", "ada@localhost"),
            ("Grace Hopper", "grace@navy.mil"),
            // A real domain says who it is: no guessing from the first name.
            ("Grace", "grace@cobol.org"),
        ]);
        assert_eq!(people["ada@raspberry.lan"], people["ada.lovelace@acme.com"]);
        assert_eq!(people["ada@localhost"], people["ada.lovelace@acme.com"]);
        assert_ne!(people["grace@cobol.org"], people["grace@navy.mil"]);
    }

    #[test]
    fn multi_word_names_win_over_more_used_words() {
        let people = people(&[
            ("countess", "ada@acme.com"),
            ("countess", "ada@acme.com"),
            ("countess", "ada@acme.com"),
            ("Ada Lovelace", "ada@acme.com"),
        ]);
        assert_eq!(shown(&people, "ada@acme.com"), "Ada");
        assert_eq!(people.list[0].source, Source::Name);
    }

    #[test]
    fn last_names_never_show() {
        let people = people(&[
            // Only a last name, known from the `first.last` email.
            ("Lovelace", "ada.lovelace@acme.com"),
            // A username starting with the last name.
            ("hopperg", "grace.hopper@navy.mil"),
            ("ALAN TURING", "at@acme.com"),
        ]);
        assert_eq!(shown(&people, "ada.lovelace@acme.com"), "Ada");
        assert_eq!(shown(&people, "grace.hopper@navy.mil"), "Grace");
        assert_eq!(people.list[1].source, Source::Email);
        // All in capitals: the first word, tidied.
        assert_eq!(shown(&people, "at@acme.com"), "Alan");
    }

    #[test]
    fn mixed_case_first_names_are_kept() {
        let people = people(&[
            ("LeVar Burton", "levar@acme.com"),
            ("jean-pierre", "jp@acme.com"),
            ("Jean-Pierre", "jp@acme.com"),
        ]);
        assert_eq!(shown(&people, "levar@acme.com"), "LeVar");
        assert_eq!(shown(&people, "jp@acme.com"), "Jean-Pierre");
    }

    #[test]
    fn machines_keep_their_name() {
        let people = people(&[
            (
                "dependabot[bot]",
                "49699333+dependabot[bot]@users.noreply.github.com",
            ),
            ("Jenkins", "jenkins@ci.acme.com"),
            ("Deploy Bot", "deploy@acme.com"),
        ]);
        assert_eq!(
            shown(&people, "49699333+dependabot[bot]@users.noreply.github.com"),
            "dependabot"
        );
        assert_eq!(shown(&people, "jenkins@ci.acme.com"), "Jenkins");
        assert_eq!(shown(&people, "deploy@acme.com"), "Deploy Bot");
        assert!(people.list.iter().all(|p| p.source == Source::Machine));
    }

    #[test]
    fn initials_tell_apart_accented_namesakes_only_with_a_last_name() {
        let people = people(&[
            ("Jérôme Martin", "jm@acme.com"),
            ("Jerome Durand", "jdurand@acme.com"),
            ("Thomas", "thomas@acme.com"),
            ("Thomas Durand", "td@acme.com"),
        ]);
        assert_eq!(shown(&people, "jm@acme.com"), "Jérôme M.");
        assert_eq!(shown(&people, "jdurand@acme.com"), "Jerome D.");
        // No last name known: the first name alone.
        assert_eq!(shown(&people, "thomas@acme.com"), "Thomas");
        assert_eq!(shown(&people, "td@acme.com"), "Thomas D.");
    }

    #[test]
    fn learning_follows_chains_of_merges() {
        let mut people = people(&[
            ("countess", "c@acme.com"),
            ("al", "al@home.org"),
            ("Augusta King", "aking@acme.com"),
        ]);
        assert_eq!(
            people.learn("aking@acme.com", Some("augusta"), None),
            Some(2)
        );
        // Same account: 1 goes into 2, whoever came first in the history.
        assert_eq!(people.learn("al@home.org", Some("augusta"), None), Some(2));
        // The profile's full name is 2's commit name: 2 (with 1) goes into 0, the earliest.
        assert_eq!(
            people.learn("c@acme.com", None, Some("Augusta King")),
            Some(0)
        );
        assert_eq!(people.take_merges(), [(1, 2), (2, 0)]);
        assert!(people.take_merges().is_empty());
        assert_eq!(people.resolve(1), 0);
        assert_eq!(people.by_email["al@home.org"], 0);
        let person = &people.list[0];
        assert_eq!(person.commits, 3);
        assert_eq!(person.emails.len(), 3);
        assert_eq!(person.login.as_deref(), Some("augusta"));
        // Those merged away are renamed too (they're left empty), but nobody shows them.
        assert!(people.name_all().contains(&0));
        assert_eq!(shown(&people, "al@home.org"), "Augusta");
    }

    #[test]
    fn learning_keeps_the_first_answers() {
        let mut people = people(&[("gh", "gh@acme.com"), ("Grace", "grace@navy.mil")]);
        assert_eq!(people.learn("nobody@acme.com", Some("x"), None), None);
        people.learn("gh@acme.com", Some("grace"), Some("  "));
        assert_eq!(people.list[0].profile, None);
        // A single-word profile name merges nobody.
        people.learn("gh@acme.com", Some("grace"), Some("Grace"));
        people.learn("gh@acme.com", Some("hopper"), Some("Amazing Grace"));
        assert!(people.take_merges().is_empty());
        assert_eq!(people.list[0].login.as_deref(), Some("grace"));
        assert_eq!(people.list[0].profile.as_deref(), Some("Grace"));
        people.name_all();
        assert_eq!(shown(&people, "gh@acme.com"), "Grace");
        assert_eq!(people.list[0].source, Source::Profile);
    }

    #[test]
    fn name_all_reports_only_changes() {
        let mut people = people(&[("x", "x@acme.com"), ("Ada Lovelace", "ada@acme.com")]);
        assert!(people.name_all().is_empty());
        people.learn("x@acme.com", Some("grace-hopper"), Some("Grace Hopper"));
        assert_eq!(people.name_all(), [0]);
        assert_eq!(shown(&people, "x@acme.com"), "Grace");
    }

    #[test]
    fn aliases_match_names_ignoring_case_and_spaces() {
        let people = People::new(
            [
                ("Ada Byron", "ada@acme.com"),
                ("countess", "lovelace@home.org"),
                ("Grace Hopper", "grace@navy.mil"),
            ]
            .into_iter(),
            &aliases(&[
                ("Ada", &["ada byron", " lovelace@home.org "]),
                ("Nobody", &["nobody@acme.com"]),
            ]),
        );
        assert_eq!(
            people.by_email["ada@acme.com"],
            people.by_email["lovelace@home.org"]
        );
        assert_eq!(shown(&people, "ada@acme.com"), "Ada");
        assert_eq!(people.list[0].source, Source::Config);
        assert_eq!(people.list[1].alias, None);
        assert_eq!(shown(&people, "grace@navy.mil"), "Grace");
    }

    #[test]
    fn aliases_match_emails_ignoring_case() {
        let people = People::new(
            [
                ("Ada Byron", "Ada.Byron@Acme.com"),
                ("countess", "lovelace@home.org"),
            ]
            .into_iter(),
            &aliases(&[("Ada", &["ada.byron@acme.com", "lovelace@home.org"])]),
        );
        assert_eq!(
            people.by_email["Ada.Byron@Acme.com"],
            people.by_email["lovelace@home.org"]
        );
        assert_eq!(shown(&people, "Ada.Byron@Acme.com"), "Ada");
    }

    #[test]
    fn the_report_lists_most_commits_first() {
        let mut people = people(&[
            ("Ada Lovelace", "ada@acme.com"),
            ("Grace Hopper", "grace@navy.mil"),
            ("Grace Hopper", "ghopper@acme.com"),
            ("grace", "ghopper@acme.com"),
        ]);
        people.learn("grace@navy.mil", Some("ghopper"), Some("Grace B. Hopper"));
        let report = people.report();
        assert!(report.starts_with("2 people behind 3 emails\n"));
        let grace = report.find("Grace ").expect("Grace in the report");
        assert!(grace < report.find("Ada ").expect("Ada in the report"));
        assert!(report.contains("3 commits  @ghopper"));
        assert!(report.contains("    Grace Hopper, grace\n"));
        assert!(report.contains("    GitHub profile: Grace B. Hopper\n"));
        assert!(report.contains("    grace@navy.mil, ghopper@acme.com\n"));
        assert!(report.contains(" name "));
    }

    #[test]
    fn one_stray_commit_merges_nobody() {
        // Ada once committed with Grace's email (a co-authored commit): still two people.
        let mut authors = vec![("Ada Lovelace", "ada@acme.com"); 40];
        authors.extend(vec![("Grace Hopper", "grace@acme.com"); 30]);
        authors.push(("Ada Lovelace", "grace@acme.com"));
        let people = group_people(authors.iter().copied(), &Aliases::new());
        assert_ne!(people["ada@acme.com"], people["grace@acme.com"]);
        // But a name that is most of an email's commits still links it.
        authors.push(("Ada Lovelace", "ada@home.org"));
        let people = group_people(authors.iter().copied(), &Aliases::new());
        assert_eq!(people["ada@acme.com"], people["ada@home.org"]);
    }

    #[test]
    fn shared_addresses_are_no_clue() {
        let people = group_people(
            [
                ("Ada Lovelace", "contact@lovelace.org"),
                ("Grace Hopper", "contact@hopper.dev"),
            ],
            &Aliases::new(),
        );
        assert_ne!(people["contact@lovelace.org"], people["contact@hopper.dev"]);
    }

    #[test]
    fn an_email_used_in_passing_says_nothing_about_its_person() {
        // Ada made a few commits with Grace's email; GitHub says that email is Grace's.
        let mut authors = vec![("Ada Lovelace", "ada@acme.com"); 40];
        authors.extend(vec![("Ada Lovelace", "grace@home.org"); 4]);
        authors.extend(vec![("Grace Hopper", "grace@acme.com"); 30]);
        let mut people = People::new(authors.iter().copied(), &Aliases::new());
        let (ada, grace) = (
            people.by_email["ada@acme.com"],
            people.by_email["grace@acme.com"],
        );
        people.learn("grace@acme.com", Some("ghopper"), None);
        people.learn("grace@home.org", Some("ghopper"), None);
        assert_ne!(people.resolve(ada), people.resolve(grace));
        assert_eq!(people.list[ada].login, None);
    }

    #[test]
    fn logins_ignore_case() {
        let mut people = people(&[
            ("Ada Lovelace", "ada@acme.com"),
            ("AdaL", "1234+adal@users.noreply.github.com"),
        ]);
        let (work, noreply) = (
            people.by_email["ada@acme.com"],
            people.by_email["1234+adal@users.noreply.github.com"],
        );
        people.learn("ada@acme.com", Some("AdaL"), None);
        people.learn("1234+adal@users.noreply.github.com", Some("adal"), None);
        assert_eq!(people.resolve(work), people.resolve(noreply));
    }

    #[test]
    fn usernames_starting_with_a_word_keep_it_whole() {
        let people = people(&[
            (
                "TheRawMeatball",
                "1+therawmeatball@users.noreply.github.com",
            ),
            ("BigCheese", "2+bigcheese@users.noreply.github.com"),
            ("AnnaKb7", "anna@acme.com"),
        ]);
        let shown = |email: &str| people.list[people.by_email[email]].display.to_lowercase();
        // Never `The` or `Big`: the username, as GitHub knows it.
        assert_eq!(
            shown("1+therawmeatball@users.noreply.github.com"),
            "therawmeatball"
        );
        assert_eq!(shown("2+bigcheese@users.noreply.github.com"), "bigcheese");
        assert_eq!(shown("anna@acme.com"), "anna");
    }
}
