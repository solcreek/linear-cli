//! Shell completion, as cobra provided it: `completion SHELL` prints a
//! script, and the script asks `linearctl __complete ARGS…` for the words
//! that can come next. The scripts are cobra's (Apache-2.0), as cobra 1.10
//! generated them for linearctl.

use crate::cli::{self, Args, Cmd, Ctx, Flag, Found, Kind};
use crate::fail::R;
use crate::run::Run;

const BASH: &str = include_str!("../completion/bash.txt");
const ZSH: &str = include_str!("../completion/zsh.txt");
const FISH: &str = include_str!("../completion/fish.txt");
const POWERSHELL: &str = include_str!("../completion/powershell.txt");

pub fn commands() -> Vec<Cmd> {
    let shell = |name: &'static str, long: &str, run: cli::Handler| {
        let mut c = Cmd::new(name, &format!("Generate the autocompletion script for {name}"))
            .long(long)
            .run(run)
            .flag(Flag::bool("no-descriptions", "disable completion descriptions"));
        // As cobra has it: only bash's usage line leaves out [flags].
        c.flags_in_use_line = name != "bash";
        c
    };
    let mut completion = Cmd::new("completion", "Generate the autocompletion script for the specified shell").long(
        "Generate the autocompletion script for linearctl for the specified shell.\n\
See each sub-command's help for details on how to use the generated script.\n",
    );
    completion.subs = vec![
        shell(
            "bash",
            "Generate the autocompletion script for the bash shell.\n\n\
This script depends on the 'bash-completion' package.\n\
If it is not installed already, you can install it via your OS's package manager.\n\n\
To load completions in your current shell session:\n\n\
\tsource <(linearctl completion bash)\n\n\
To load completions for every new session, execute once:\n\n\
#### Linux:\n\n\
\tlinearctl completion bash > /etc/bash_completion.d/linearctl\n\n\
#### macOS:\n\n\
\tlinearctl completion bash > $(brew --prefix)/etc/bash_completion.d/linearctl\n\n\
You will need to start a new shell for this setup to take effect.\n",
            |run, ctx| script(run, ctx, BASH),
        ),
        shell(
            "zsh",
            "Generate the autocompletion script for the zsh shell.\n\n\
If shell completion is not already enabled in your environment you will need\n\
to enable it.  You can execute the following once:\n\n\
\techo \"autoload -U compinit; compinit\" >> ~/.zshrc\n\n\
To load completions in your current shell session:\n\n\
\tsource <(linearctl completion zsh)\n\n\
To load completions for every new session, execute once:\n\n\
#### Linux:\n\n\
\tlinearctl completion zsh > \"${fpath[1]}/_linearctl\"\n\n\
#### macOS:\n\n\
\tlinearctl completion zsh > $(brew --prefix)/share/zsh/site-functions/_linearctl\n\n\
You will need to start a new shell for this setup to take effect.\n",
            |run, ctx| script(run, ctx, ZSH),
        ),
        shell(
            "fish",
            "Generate the autocompletion script for the fish shell.\n\n\
To load completions in your current shell session:\n\n\
\tlinearctl completion fish | source\n\n\
To load completions for every new session, execute once:\n\n\
\tlinearctl completion fish > ~/.config/fish/completions/linearctl.fish\n\n\
You will need to start a new shell for this setup to take effect.\n",
            |run, ctx| script(run, ctx, FISH),
        ),
        shell(
            "powershell",
            "Generate the autocompletion script for powershell.\n\n\
To load completions in your current shell session:\n\n\
\tlinearctl completion powershell | Out-String | Invoke-Expression\n\n\
To load completions for every new session, add the output of the above command\n\
to your powershell profile.\n",
            |run, ctx| script(run, ctx, POWERSHELL),
        ),
    ];
    let mut complete = Cmd::new("__complete [command-line]", "Request shell completion choices for the specified command-line")
        .long(
            "__complete is a special command that is used by the shell completion logic\n\
to request completion choices for the specified command-line.",
        )
        .alias("__completeNoDesc")
        .args(Args::AtLeast(1))
        .run(complete);
    complete.hidden = true;
    complete.parse_flags = false;
    complete.flags_in_use_line = false;
    vec![completion, complete]
}

fn script(run: &mut Run, ctx: &Ctx, text: &str) -> R {
    if ctx.bool("no-descriptions") {
        return run.print(&text.replacen(" __complete ", " __completeNoDesc ", 1));
    }
    run.print(text)
}

const DEFAULT: u8 = 0;
const NO_FILE_COMP: u8 = 4;

/// The words that can come next on a command line, one per line with its
/// description, then :DIRECTIVE.
fn complete(run: &mut Run, ctx: &Ctx) -> R {
    let descriptions = ctx.found.called_as != "__completeNoDesc";
    let (to_complete, args) = ctx.args.split_last().expect("at least one arg");
    let root = super::root();
    let found = cli::found_at(&root, args);
    let (words, directive) = completions(&found, args, to_complete);
    let mut out = String::new();
    for (word, desc) in words {
        let mut line = if descriptions && !desc.is_empty() { format!("{word}\t{desc}") } else { word };
        if let Some(i) = line.find('\n') {
            line.truncate(i);
        }
        out.push_str(line.trim());
        out.push('\n');
    }
    out.push_str(&format!(":{directive}\n"));
    run.print(&out)?;
    let name = if directive == NO_FILE_COMP { "ShellCompDirectiveNoFileComp" } else { "ShellCompDirectiveDefault" };
    run.eprint(&format!("Completion ended with directive: {name}\n"));
    Ok(())
}

fn completions(found: &Found, args: &[String], to_complete: &str) -> (Vec<(String, String)>, u8) {
    let cmd = found.cmd();
    let flags: Vec<Flag> = found.inherited_flags().into_iter().chain(found.local_flags()).collect();
    // What the arguments before the word already say: flags given, and
    // whether the word is a flag's value.
    let mut changed: Vec<&str> = vec![];
    let mut positional = 0;
    let mut value_of_flag = false;
    let path_len = found.chain.len() - 1;
    let mut skip_names = path_len;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if let Some(name) = a.strip_prefix("--") {
            let (name, has_value) = name.split_once('=').map_or((name, false), |(n, _)| (n, true));
            if let Some(f) = flags.iter().find(|f| f.name == name) {
                changed.push(f.name);
                if !has_value && f.no_opt.is_none() {
                    if i == args.len() {
                        value_of_flag = true;
                    }
                    i += 1;
                }
            }
        } else if a.starts_with('-') && a.len() > 1 {
            if let Some(f) = flags.iter().find(|f| a[1..].starts_with(f.short.unwrap_or('\0'))) {
                changed.push(f.name);
            }
        } else if skip_names > 0 {
            skip_names -= 1;
        } else {
            positional += 1;
        }
    }
    if value_of_flag || (to_complete.starts_with('-') && to_complete.contains('=')) {
        return (vec![], DEFAULT);
    }
    if to_complete.starts_with('-') {
        let mut words = vec![];
        for f in &flags {
            if f.hidden || (changed.contains(&f.name) && f.kind != Kind::Strs) {
                continue;
            }
            let long = format!("--{}", f.name);
            if long.starts_with(to_complete) {
                words.push((long, f.usage.clone()));
            }
            if let Some(c) = f.short {
                let short = format!("-{c}");
                if short.starts_with(to_complete) {
                    words.push((short, f.usage.clone()));
                }
            }
        }
        return (words, NO_FILE_COMP);
    }
    let mut words = vec![];
    let mut directive = DEFAULT;
    let mut subs: Vec<&Cmd> = cmd.subs.iter().collect();
    subs.sort_by(|a, b| a.name().cmp(b.name()));
    if positional == 0 {
        for s in &subs {
            if s.available() || s.help_command {
                if s.name().starts_with(to_complete) {
                    words.push((s.name().to_string(), s.short.clone()));
                }
                directive = NO_FILE_COMP;
            }
        }
    }
    match cmd.name() {
        // help completes the commands it can explain.
        "help" if cmd.help_command => {
            let root = super::root();
            let target = cli::found_at(&root, &args[path_len..]);
            let mut subs: Vec<&Cmd> = target.cmd().subs.iter().collect();
            subs.sort_by(|a, b| a.name().cmp(b.name()));
            for s in subs {
                if (s.available() || s.help_command) && s.name().starts_with(to_complete) {
                    words.push((s.name().to_string(), s.short.clone()));
                }
            }
            directive = NO_FILE_COMP;
        }
        "completion" | "bash" | "zsh" | "fish" | "powershell" => directive = NO_FILE_COMP,
        _ => {}
    }
    (words, directive)
}
