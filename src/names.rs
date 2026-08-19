use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{git, registry};

/// Branch names so creating a workspace does not stall on naming it. Composers,
/// because they are short, memorable, and distinct from each other at a glance.
const NAMES: [&str; 96] = [
    "bach",
    "handel",
    "vivaldi",
    "purcell",
    "telemann",
    "scarlatti",
    "corelli",
    "albinoni",
    "rameau",
    "monteverdi",
    "strozzi",
    "hildegard",
    "haydn",
    "mozart",
    "beethoven",
    "schubert",
    "clementi",
    "boccherini",
    "gluck",
    "farrenc",
    "chopin",
    "liszt",
    "schumann",
    "clara",
    "mendelssohn",
    "fanny",
    "brahms",
    "wagner",
    "verdi",
    "berlioz",
    "bruckner",
    "dvorak",
    "grieg",
    "smetana",
    "franck",
    "bizet",
    "gounod",
    "offenbach",
    "rossini",
    "bellini",
    "donizetti",
    "paganini",
    "tchaikovsky",
    "mussorgsky",
    "borodin",
    "rimsky",
    "glinka",
    "chaminade",
    "mahler",
    "strauss",
    "sibelius",
    "nielsen",
    "elgar",
    "holst",
    "delius",
    "faure",
    "chausson",
    "duparc",
    "debussy",
    "ravel",
    "satie",
    "dukas",
    "beach",
    "smyth",
    "boulanger",
    "rachmaninov",
    "scriabin",
    "prokofiev",
    "shostakovich",
    "stravinsky",
    "bartok",
    "kodaly",
    "janacek",
    "enescu",
    "szymanowski",
    "schoenberg",
    "berg",
    "webern",
    "hindemith",
    "weill",
    "korngold",
    "britten",
    "walton",
    "tippett",
    "messiaen",
    "poulenc",
    "milhaud",
    "honegger",
    "dutilleux",
    "boulez",
    "ligeti",
    "kurtag",
    "part",
    "glass",
    "gubaidulina",
    "saariaho",
];

fn seed() -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0)
}

/// The first name not already taken by a workspace, worktree, or branch in this
/// repo, starting from an arbitrary point so successive workspaces differ.
pub fn suggest(root: &Path) -> String {
    let repo = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let start = seed() % NAMES.len();

    for i in 0..NAMES.len() {
        let name = NAMES[(start + i) % NAMES.len()];
        if registry::exists(&format!("{repo}--{name}")) {
            continue;
        }
        if git::worktree_path(root, name).exists() {
            continue;
        }
        if git::branch_exists(root, name) {
            continue;
        }
        return name.to_string();
    }

    // Every name is in use: fall back rather than block the form.
    format!("{}-{}", NAMES[start], seed() % 997)
}
