//! The command tree, as `linearctl describe` lists it.

mod comment;
mod issue;
mod lookups;
mod project;
mod relation;

use crate::cli::{self, Args, Cmd, Ctx, Flag};
use crate::fail::R;
use crate::run::{Run, VERSION};

pub fn root() -> Cmd {
    let env_on = |name: &str| if std::env::var(name).is_ok_and(|v| v == "1") { "true" } else { "false" };
    let mut root = Cmd::new("linearctl", "An agent-first CLI for Linear")
        .long(
            "An agent-first CLI for Linear: every command takes --json, every change takes\n\
--dry-run or --yes, and 'linearctl describe' prints the whole surface as JSON.\n\n\
The API key comes from LINEAR_API_KEY, else from 'linearctl auth login'.\n\
LINEARCTL_HOME moves config and credentials under one directory.",
        )
        .example("  linearctl issue list --assignee me --json\n  linearctl describe")
        .run(|run, ctx| {
            run.help(ctx);
            Ok(())
        })
        // Our own --version, so there is no -v: elsewhere -v means verbose.
        .flag(Flag::bool("version", "print the version"));
    root.persistent = vec![
        Flag::bool("json", "print one JSON document on stdout"),
        Flag::whole("timeout", 30, "give up on a network request after `N` seconds"),
        Flag::bool("demo", "use a built-in fictional workspace: no key, no network; or LINEARCTL_DEMO=1")
            .default_to(env_on("LINEARCTL_DEMO")),
        Flag::bool("meta", "add meta (requests sent, rate limit left) to --json; or LINEARCTL_META=1")
            .default_to(env_on("LINEARCTL_META")),
    ];
    let mut subs: Vec<Cmd> = vec![issue::command(), comment::command(), relation::command()];
    for mut c in lookups::commands() {
        if c.name() == "project" {
            c.short = "List, show and update projects".into();
            c.subs.extend(project::commands());
        }
        subs.push(c);
    }
    subs.push(
        Cmd::new("describe", "Print every command, flag and exit code as JSON")
            .example("  linearctl describe")
            .run(describe),
    );
    subs.push(help_command());
    root.subs = subs;
    root
}

fn describe(run: &mut Run, _: &Ctx) -> R {
    let root = root();
    let d = cli::describe(&cli::root_found(&root), VERSION);
    run.print_json(d)
}

/// cobra's help command: `linearctl help issue show`.
fn help_command() -> Cmd {
    let mut c = Cmd::new("help [command]", "Help about any command")
        .long("Help provides help for any command in the application.\nSimply type linearctl help [path to command] for full details.")
        .args(Args::Legacy)
        .run(|run, ctx| {
            let root = root();
            let found = cli::found_at(&root, &ctx.args);
            let text = cli::help(&found);
            run.print(&text)
        });
    c.help_command = true;
    c
}
