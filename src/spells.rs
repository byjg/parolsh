//! Spells: your recurring operations, each a shell script and the
//! instructions around it, in `~/.config/parolsh/spells/<name>/spell.toml`.
//! The agent gets them as an MCP server, `parolsh-spells` (`parolsh mcp
//! --spells`), in every conversation: it finds the spell for what you ask,
//! follows its instructions, and casts it. The spells are read at each
//! call, so one saved during a conversation is there at once. The script is
//! the predictable part; the instructions are the judgement around it.
//!
//! Global only: a project's `.parolsh/` cannot define them, as it cannot
//! choose the shell, so a cloned repository adds no code the agent may run.

use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{config, mcp, shell, ui};

/// The server's name, as the agent shows its tools (`parolsh-spells`).
pub const NAME: &str = "parolsh-spells";

/// Each spell's file, in its own directory.
const FILE: &str = "spell.toml";

/// How much of a script's output `cast_spell` returns, its end: as `!+`.
const OUTPUT_LIMIT: usize = 16 * 1024;

/// `spell.toml` as written on disk.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpellFile {
    description: String,
    run: Option<String>,
    #[serde(default)]
    args: Vec<Arg>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    instructions: Option<String>,
}

/// An argument of the script, given to it in order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arg {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spell {
    /// Its directory's name.
    pub name: String,
    pub dir: PathBuf,
    pub description: String,
    /// The script, relative to `dir`; none for a procedure the agent
    /// follows with its own tools.
    pub run: Option<String>,
    pub args: Vec<Arg>,
    /// Added to the script's environment.
    pub env: BTreeMap<String, String>,
    pub instructions: Option<String>,
}

impl Spell {
    fn read(dir: &Path, name: &str) -> anyhow::Result<Self> {
        let path = dir.join(FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let file: SpellFile =
            toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))?;
        if file.run.is_none() && file.instructions.is_none() {
            bail!("{} has neither `run` nor `instructions`", path.display());
        }
        Ok(Self {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            description: file.description,
            run: file.run,
            args: file.args,
            env: file.env,
            instructions: file.instructions,
        })
    }

    /// The script's path, when it has one.
    pub fn script(&self) -> Option<PathBuf> {
        self.run.as_ref().map(|run| self.dir.join(run))
    }

    /// Its name and arguments: `vs-install-mod <file>`.
    pub fn usage(&self) -> String {
        let mut usage = self.name.clone();
        for arg in &self.args {
            usage.push_str(&format!(" <{}>", arg.name));
        }
        usage
    }

    /// Starts the script with `args`, through `shell`, in `cwd`.
    pub fn start(
        &self,
        args: &[String],
        shell: &[String],
        cwd: &Path,
    ) -> anyhow::Result<shell::Captive> {
        let Some(script) = self.script() else {
            bail!(
                "spell `{}` has no script: follow its instructions with your own tools",
                self.name
            );
        };
        let executable = std::fs::metadata(&script)
            .with_context(|| format!("cannot run {}", script.display()))?
            .permissions()
            .mode()
            & 0o111
            != 0;
        if !executable {
            bail!(
                "{} is not executable: `chmod +x` it first",
                script.display()
            );
        }
        if args.len() != self.args.len() {
            let names: Vec<&str> = self.args.iter().map(|arg| arg.name.as_str()).collect();
            bail!(
                "spell `{}` takes {} argument(s) ({}), got {}",
                self.name,
                names.len(),
                names.join(", "),
                args.len()
            );
        }
        Ok(shell::start_captured(
            shell,
            &script,
            args,
            cwd,
            &self.env,
            OUTPUT_LIMIT,
        )?)
    }
}

/// `$XDG_CONFIG_HOME/parolsh/spells`, or `~/.config/parolsh/spells`.
pub fn dir() -> Option<PathBuf> {
    Some(config::global_path()?.parent()?.join("spells"))
}

/// The spells in `dir`, by name, and what is wrong with the ones that cannot
/// be read. A directory without `spell.toml` is not a spell.
pub fn load(dir: &Path) -> (Vec<Spell>, Vec<String>) {
    let mut spells = Vec::new();
    let mut problems = Vec::new();
    for (name, path) in spell_dirs(dir) {
        match Spell::read(&path, &name) {
            Ok(spell) => spells.push(spell),
            Err(e) => problems.push(format!("{e:#}")),
        }
    }
    (spells, problems)
}

/// The names of the spells in `dir`, with their descriptions, for `Tab`.
pub fn listed(dir: &Path) -> Vec<(String, String)> {
    load(dir)
        .0
        .into_iter()
        .map(|spell| (spell.name, spell.description))
        .collect()
}

/// The directories in `dir` that have a `spell.toml`, by name.
fn spell_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(_, path)| path.join(FILE).is_file())
        .collect();
    found.sort();
    found
}

/// A spell still running after `cast_spell` stopped waiting for it, as its
/// server writes it in `jobs/<id>.json` of the [`Here`] directory: Parolsh
/// lists it, says when it ended, and stops it with the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFile {
    pub id: u32,
    pub spell: String,
    pub args: Vec<String>,
    /// Its process group.
    pub pgid: i32,
    /// When it started, in seconds since the epoch.
    pub started: u64,
    /// Set when it ended, once its output is all read.
    pub exit: Option<i32>,
}

impl JobFile {
    pub fn running(&self) -> bool {
        self.exit.is_none() && shell::group_alive(self.pgid)
    }

    /// `job 2 · vs-install-mod mod.zip`.
    pub fn label(&self) -> String {
        let mut label = format!("spell job {} · {}", self.id, self.spell);
        for arg in &self.args {
            label.push(' ');
            label.push_str(arg);
        }
        label
    }

    /// How long it has run.
    pub fn elapsed(&self) -> Duration {
        let started = UNIX_EPOCH + Duration::from_secs(self.started);
        SystemTime::now()
            .duration_since(started)
            .unwrap_or_default()
    }

    /// How it ended: `exit 0`.
    pub fn outcome(&self) -> String {
        format!("exit {}", self.exit.unwrap_or_default())
    }
}

/// What Parolsh shares with the spells' server, in a directory of its own
/// while it runs: `cwd`, where your commands run, which a `cd` changes
/// during a conversation, after the agent started the server, and `jobs/`,
/// the spells that run in the background. Parolsh writes `cwd` each time it
/// changes, and the server reads it on each call. The directory goes, and
/// the spells still running stop, when Parolsh ends.
pub struct Here {
    dir: PathBuf,
}

impl Here {
    /// The directory of this Parolsh process in `base`, holding `cwd`.
    pub fn create(base: &Path, cwd: &Path) -> Option<Self> {
        let dir = base.join(std::process::id().to_string());
        std::fs::create_dir_all(dir.join(JOBS)).ok()?;
        let here = Self { dir };
        here.set(cwd);
        Some(here)
    }

    pub fn set(&self, cwd: &Path) {
        let _ = std::fs::write(self.dir.join(CWD), cwd.as_os_str().as_encoded_bytes());
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// The spells in the background, running or ended, by number.
    pub fn jobs(&self) -> Vec<JobFile> {
        read_jobs(&self.dir)
    }

    /// The ones still running. One whose server was killed before it could
    /// write the end is neither running nor ended: it goes with Parolsh.
    pub fn running(&self) -> Vec<JobFile> {
        self.jobs().into_iter().filter(JobFile::running).collect()
    }

    /// The jobs that ended since the last call: each is given once.
    pub fn take_ended(&self) -> Vec<JobFile> {
        let ended: Vec<JobFile> = self
            .jobs()
            .into_iter()
            .filter(|job| job.exit.is_some())
            .collect();
        for job in &ended {
            let _ = std::fs::remove_file(job_path(&self.dir, job.id));
        }
        ended
    }

    /// Stops the spells still running, with all they started, and forgets
    /// them: the conversation that cast them is over.
    pub fn stop_running(&self) {
        for job in self.running() {
            shell::stop_group(job.pgid);
            let _ = std::fs::remove_file(job_path(&self.dir, job.id));
        }
    }
}

impl Drop for Here {
    fn drop(&mut self) {
        self.stop_running();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// In a [`Here`] directory: where your commands run, and the jobs.
const CWD: &str = "cwd";
const JOBS: &str = "jobs";

/// The jobs of the [`Here`] directory `here`, by number.
fn read_jobs(here: &Path) -> Vec<JobFile> {
    let Ok(entries) = std::fs::read_dir(here.join(JOBS)) else {
        return Vec::new();
    };
    let mut jobs: Vec<JobFile> = entries
        .flatten()
        .filter_map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok())
        .collect();
    jobs.sort_by_key(|job| job.id);
    jobs
}

fn job_path(here: &Path, id: u32) -> PathBuf {
    here.join(JOBS).join(format!("{id}.json"))
}

/// The directory a [`Here`] at `path` holds, when it still exists.
fn read_here(path: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(path.join(CWD)).ok()?;
    let dir = PathBuf::from(OsString::from_vec(bytes));
    dir.is_dir().then_some(dir)
}

/// Writes job `job` in `here`, under the first number after `last` that no
/// other job of this Parolsh has: the agent may run several servers, one
/// per conversation it keeps. Returns the number.
fn add_job(here: &Path, mut job: JobFile, last: u32) -> std::io::Result<u32> {
    let on_disk = read_jobs(here).iter().map(|job| job.id).max().unwrap_or(0);
    let mut id = 1 + last.max(on_disk);
    loop {
        job.id = id;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(job_path(here, id))
        {
            Ok(mut file) => {
                file.write_all(&serde_json::to_vec(&job)?)?;
                return Ok(id);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => id += 1,
            Err(e) => return Err(e),
        }
    }
}

/// The MCP server Parolsh gives the agent: this program, sharing `here` with
/// Parolsh. `None` when Parolsh cannot tell where its own program is.
pub fn server(here: Option<&Path>) -> Option<McpServer> {
    let program = std::env::current_exe().ok()?;
    let mut args = vec!["mcp".to_string(), "--spells".to_string()];
    if let Some(here) = here {
        args.extend(["--here".to_string(), here.display().to_string()]);
    }
    Some(McpServer::Stdio(
        McpServerStdio::new(NAME, program).args(args),
    ))
}

/// How long `cast_spell` waits for a script before it goes on in the
/// background, as a job: agents give up on a tool call after a while, and
/// the user gets the prompt back.
pub const WAIT: Duration = Duration::from_secs(30);

/// How long a stopped script has to end after SIGTERM, before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

/// A spell that went on in the background.
struct Job {
    id: u32,
    label: String,
    started: Instant,
    captive: Arc<shell::Captive>,
}

/// The spells' tools.
pub struct SpellTools {
    /// Where the spells are.
    pub dir: PathBuf,
    /// The configured shell, which runs the scripts.
    pub shell: Vec<String>,
    /// Parolsh's [`Here`] directory: without it, the scripts run in the
    /// server's directory, the agent's, and Parolsh does not see the jobs.
    pub here: Option<PathBuf>,
    /// How long `cast_spell` waits: [`WAIT`].
    pub wait: Duration,
    jobs: Mutex<Vec<Job>>,
}

impl SpellTools {
    pub fn new(dir: PathBuf, shell: Vec<String>, here: Option<PathBuf>) -> Self {
        Self {
            dir,
            shell,
            here,
            wait: WAIT,
            jobs: Mutex::default(),
        }
    }

    /// Where your commands run now.
    fn cwd(&self) -> anyhow::Result<PathBuf> {
        match self.here.as_deref().and_then(read_here) {
            Some(dir) => Ok(dir),
            None => Ok(std::env::current_dir()?),
        }
    }

    fn find(&self, name: &str) -> anyhow::Result<Spell> {
        let (spells, problems) = load(&self.dir);
        if let Some(spell) = spells.into_iter().find(|spell| spell.name == name) {
            return Ok(spell);
        }
        let file = self.dir.join(name).join(FILE);
        match problems
            .into_iter()
            .find(|problem| problem.contains(&*file.to_string_lossy()))
        {
            Some(problem) => bail!("{problem}"),
            None => bail!("no spell `{name}`: list_spells lists them"),
        }
    }

    /// Runs spell `name`: its result when it ends within [`Self::wait`],
    /// else it goes on as a job.
    fn cast(&self, name: &str, args: Vec<String>) -> anyhow::Result<String> {
        let spell = self.find(name)?;
        let captive = Arc::new(spell.start(&args, &self.shell, &self.cwd()?)?);
        if let Some(code) = captive.wait(Some(self.wait)) {
            return Ok(result(code, &captive.output()));
        }
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
            .saturating_sub(self.wait.as_secs());
        let mut file = JobFile {
            id: 0,
            spell: spell.name.clone(),
            args,
            pgid: captive.pgid(),
            started,
            exit: None,
        };
        let mut jobs = self.jobs.lock().expect("jobs lock");
        // After this server's jobs too: Parolsh removes the file of one it
        // said ended, which spell_job still shows.
        let last = jobs.iter().map(|job| job.id).max().unwrap_or(0);
        file.id = match &self.here {
            Some(here) => add_job(here, file.clone(), last)?,
            None => last + 1,
        };
        // Its end, for Parolsh, unless Parolsh forgot it: it stopped it.
        if let Some(here) = self.here.clone() {
            let ended = captive.clone();
            let mut file = file.clone();
            std::thread::spawn(move || {
                file.exit = ended.wait(None);
                let opened = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(job_path(&here, file.id));
                if let (Ok(mut opened), Ok(json)) = (opened, serde_json::to_vec(&file)) {
                    let _ = opened.write_all(&json);
                }
            });
        }
        jobs.push(Job {
            id: file.id,
            label: file.label(),
            started: Instant::now() - self.wait,
            captive: captive.clone(),
        });
        Ok(format!(
            "Still running after {} s: it goes on in the background as job {}. \
             spell_job {} shows how it goes and its output, stop_spell {} stops it. The \
             user sees at their prompt when it ends, and you are told with their next \
             message.\nOutput so far:\n{}",
            self.wait.as_secs(),
            file.id,
            file.id,
            file.id,
            captive.output().text
        ))
    }

    /// One line on job `job`: what it is, and how it is going.
    fn status(job: &Job) -> String {
        let clock = ui::clock(job.started.elapsed());
        match job.captive.wait(Some(Duration::ZERO)) {
            Some(code) => format!("{} · ended: exit {code}", job.label),
            None => format!("{} · running for {clock}", job.label),
        }
    }

    fn job_number(arguments: &Value) -> anyhow::Result<u32> {
        arguments
            .get("job")
            .and_then(Value::as_u64)
            .map(|job| job as u32)
            .ok_or_else(|| anyhow::anyhow!("`job` is required"))
    }
}

impl Drop for SpellTools {
    /// The conversation is over: its spells stop with it.
    fn drop(&mut self) {
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.iter() {
                if job.captive.wait(Some(Duration::ZERO)).is_none() {
                    shell::stop_group(job.captive.pgid());
                }
            }
        }
    }
}

/// What `cast_spell` returns of a script that ended.
fn result(code: i32, captured: &shell::Captured) -> String {
    let mut out = format!("exit {code}\n");
    if captured.truncated {
        out.push_str(&format!("(only the last {} KB)\n", OUTPUT_LIMIT / 1024));
    }
    out.push_str(&captured.text);
    out
}

impl mcp::Tools for SpellTools {
    fn name(&self) -> &'static str {
        NAME
    }

    fn instructions(&self) -> &'static str {
        "The user's spells: operations they do again and again, each a script and the \
         instructions around it. When the user asks for something a spell does, read it \
         with get_spell, follow its instructions, and run its script with cast_spell, \
         instead of improvising the steps. The user can also ask you to save what you just \
         did as a spell: list_spells says how."
    }

    fn list(&self) -> Value {
        let dir = self.dir.display();
        json!([
            {
                "name": "list_spells",
                "description": format!(
                    "The user's spells: each one's name, arguments and description. \
                     A spell is a directory {dir}/<name>/ with a spell.toml: \
                     `description` (one line: what it does, and on what), \
                     `run` (a script, relative to that directory, executable: chmod +x), \
                     `args` (`[{{ name = \"file\", description = \"...\" }}]`, given to the \
                     script in order), `env` (a table of variables for the script), \
                     `instructions` (what to check and decide before running, and what to \
                     offer when it fails). A spell may have only `run` or only \
                     `instructions`. The script takes paths as arguments and decides \
                     nothing about where files are, so that it works without Parolsh too; \
                     it gets no input, so questions go in the instructions, for you to ask. \
                     To save an operation as a spell when the user asks, write these files \
                     with your own file tools; it can be cast at once."
                ),
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": "get_spell",
                "description": "One spell: where the user's commands run, its arguments, \
                    its environment, its instructions and its script's text. Read it, and \
                    follow the instructions, before cast_spell.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The spell's name"},
                    },
                    "required": ["name"],
                },
            },
            {
                "name": "cast_spell",
                "description": format!(
                    "Runs a spell's script with its arguments, each passed as it is (never \
                     through a shell line), in the directory of the user's commands. Waits \
                     up to {} s: returns the exit code and the end of the output (16 KB). \
                     A script still running then goes on in the background as a job, with \
                     spell_job and stop_spell.",
                    self.wait.as_secs()
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The spell's name"},
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "The script's arguments, in order",
                        },
                    },
                    "required": ["name"],
                },
            },
            {
                "name": "spell_job",
                "description": "How a spell that went on in the background is going: \
                    running or ended, for how long, the exit code, and its output so far \
                    (16 KB). Without `job`, the jobs of this conversation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job": {"type": "integer", "description": "The job's number"},
                    },
                },
            },
            {
                "name": "stop_spell",
                "description": "Stops a spell that runs in the background, with all it \
                    started, when the user asks: SIGTERM, then SIGKILL after 5 s. Returns \
                    its exit code and output.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "job": {"type": "integer", "description": "The job's number"},
                    },
                    "required": ["job"],
                },
            },
        ])
    }

    fn call(&self, name: &str, arguments: &Value) -> anyhow::Result<String> {
        let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("");
        let mut out = String::new();
        match name {
            "list_spells" => {
                let (spells, problems) = load(&self.dir);
                if spells.is_empty() {
                    out.push_str("No spells yet.\n");
                }
                for spell in spells {
                    out.push_str(&format!("{}: {}\n", spell.usage(), spell.description));
                }
                for problem in problems {
                    out.push_str(&format!("(cannot be used: {problem})\n"));
                }
            }
            "get_spell" => {
                let spell = self.find(text("name"))?;
                out.push_str(&format!("{}: {}\n", spell.usage(), spell.description));
                out.push_str(&format!(
                    "The user's commands run in {}: cast_spell runs the script there.\n",
                    self.cwd()?.display()
                ));
                if !spell.args.is_empty() {
                    out.push_str("\nArguments, in order:\n");
                    for arg in &spell.args {
                        out.push_str(&format!("- {}: {}\n", arg.name, arg.description));
                    }
                }
                if !spell.env.is_empty() {
                    out.push_str("\nEnvironment of the script:\n");
                    for (key, value) in &spell.env {
                        out.push_str(&format!("- {key}={value}\n"));
                    }
                }
                if let Some(instructions) = &spell.instructions {
                    out.push_str(&format!("\nInstructions:\n{}\n", instructions.trim_end()));
                }
                match spell.script() {
                    Some(script) => {
                        let source = std::fs::read_to_string(&script)
                            .unwrap_or_else(|e| format!("(cannot read it: {e})"));
                        out.push_str(&format!(
                            "\nScript {}:\n```\n{}\n```\n",
                            script.display(),
                            source.trim_end()
                        ));
                    }
                    None => {
                        out.push_str("\nNo script: follow the instructions with your own tools.\n")
                    }
                }
            }
            "cast_spell" => {
                let args: Vec<String> = match arguments.get("args") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::Array(args)) => args
                        .iter()
                        .map(|arg| match arg {
                            Value::String(arg) => Ok(arg.clone()),
                            other => anyhow::bail!("each argument is a string, not {other}"),
                        })
                        .collect::<anyhow::Result<_>>()?,
                    Some(other) => bail!("`args` is a list of strings, not {other}"),
                };
                out = self.cast(text("name"), args)?;
            }
            "spell_job" => {
                let jobs = self.jobs.lock().expect("jobs lock");
                match arguments.get("job") {
                    None | Some(Value::Null) => {
                        if jobs.is_empty() {
                            out.push_str("No spells in the background.\n");
                        }
                        for job in jobs.iter() {
                            out.push_str(&format!("{}\n", Self::status(job)));
                        }
                    }
                    Some(_) => {
                        let id = Self::job_number(arguments)?;
                        let job = jobs
                            .iter()
                            .find(|job| job.id == id)
                            .ok_or_else(|| anyhow::anyhow!("no spell job {id} here"))?;
                        let captured = job.captive.output();
                        out.push_str(&format!("{}\n", Self::status(job)));
                        if captured.truncated {
                            out.push_str(&format!("(only the last {} KB)\n", OUTPUT_LIMIT / 1024));
                        }
                        out.push_str(&captured.text);
                    }
                }
            }
            "stop_spell" => {
                let id = Self::job_number(arguments)?;
                let captive = {
                    let jobs = self.jobs.lock().expect("jobs lock");
                    let job = jobs
                        .iter()
                        .find(|job| job.id == id)
                        .ok_or_else(|| anyhow::anyhow!("no spell job {id} here"))?;
                    job.captive.clone()
                };
                if let Some(code) = captive.wait(Some(Duration::ZERO)) {
                    out.push_str(&format!("Job {id} had already ended. "));
                    out.push_str(&result(code, &captive.output()));
                } else {
                    let code = captive.stop(GRACE).unwrap_or(-1);
                    out.push_str(&format!("Job {id} stopped. "));
                    out.push_str(&result(code, &captive.output()));
                }
            }
            _ => bail!("unknown tool `{name}`"),
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Tools;

    fn bash() -> Vec<String> {
        ["bash", "-c"].map(String::from).to_vec()
    }

    /// Writes spell `name` in `dir`: its `spell.toml`, and `run.sh`
    /// executable when there is a script.
    fn spell(dir: &Path, name: &str, toml: &str, script: Option<&str>) {
        let spell = dir.join(name);
        std::fs::create_dir_all(&spell).unwrap();
        std::fs::write(spell.join(FILE), toml).unwrap();
        if let Some(script) = script {
            let path = spell.join("run.sh");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn tools(dir: &Path, here: Option<PathBuf>) -> SpellTools {
        SpellTools::new(dir.to_path_buf(), bash(), here)
    }

    fn cast(tools: &SpellTools, name: &str, args: Value) -> anyhow::Result<String> {
        tools.call("cast_spell", &json!({"name": name, "args": args}))
    }

    /// Waits until `done` holds, for at most 10 seconds.
    fn eventually(done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn spells_are_read_by_name_and_the_broken_ones_are_said() {
        let dir = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "vs-install-mod",
            r#"
description = "Install a mod"
run = "run.sh"
args = [{ name = "file", description = "The .zip" }]
env = { VS_HOST = "jg@host" }
instructions = "Check the version first."
"#,
            Some("#!/bin/sh\n"),
        );
        spell(
            dir.path(),
            "clean-branches",
            "description = \"Clean merged branches\"\ninstructions = \"git branch --merged\"\n",
            None,
        );
        spell(dir.path(), "empty", "description = \"Nothing\"\n", None);
        spell(
            dir.path(),
            "typo",
            "descripton = \"x\"\nrun = \"a\"\n",
            None,
        );
        // Not spells: a directory without spell.toml, and a file.
        std::fs::create_dir(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("README"), "").unwrap();

        let (spells, problems) = load(dir.path());

        let names: Vec<&str> = spells.iter().map(|spell| spell.name.as_str()).collect();
        assert_eq!(names, ["clean-branches", "vs-install-mod"]);
        let mod_spell = &spells[1];
        assert_eq!(mod_spell.usage(), "vs-install-mod <file>");
        assert_eq!(
            mod_spell.script(),
            Some(dir.path().join("vs-install-mod/run.sh"))
        );
        assert_eq!(mod_spell.env["VS_HOST"], "jg@host");
        assert_eq!(spells[0].script(), None);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("neither `run` nor `instructions`"),
            "{problems:?}"
        );
        assert!(problems[1].contains("typo/spell.toml"), "{problems:?}");
    }

    /// The arguments are the script's own argv entries: nothing in them is
    /// read by a shell. The spell's environment is added, and it runs where
    /// the user's commands run, as the `Here` directory says at that moment.
    #[test]
    fn a_cast_passes_the_arguments_as_they_are_where_the_user_is() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "show",
            "description = \"Show\"\nrun = \"run.sh\"\nenv = { GREETING = \"hi\" }\n\
             args = [{ name = \"a\" }, { name = \"b\" }]\n",
            Some("#!/bin/sh\nprintf '%s|' \"$1\" \"$2\" \"$GREETING\" \"$(pwd)\"\necho oops >&2\n"),
        );
        let here = Here::create(state.path(), dir.path()).unwrap();
        let tools = tools(dir.path(), Some(here.path().to_path_buf()));
        here.set(work.path());

        let out = cast(&tools, "show", json!(["two words", "$(touch pwned)"])).unwrap();

        let cwd = work.path().canonicalize().unwrap();
        assert_eq!(
            out,
            format!(
                "exit 0\ntwo words|$(touch pwned)|hi|{}|oops\n",
                cwd.display()
            )
        );
        assert!(!work.path().join("pwned").exists());
        // The directory is read at each call.
        here.set(dir.path());
        let out = cast(&tools, "show", json!(["a", "b"])).unwrap();
        assert!(out.contains(&format!(
            "|{}|",
            dir.path().canonicalize().unwrap().display()
        )));
        // A spell that ended within the wait is no job.
        assert!(here.jobs().is_empty());
    }

    #[test]
    fn a_cast_returns_the_exit_code_and_the_end_of_the_output() {
        let dir = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "fail",
            "description = \"Fail\"\nrun = \"run.sh\"\n",
            Some("#!/bin/sh\nseq 1 10000\necho broken >&2\nexit 3\n"),
        );
        let tools = tools(dir.path(), None);

        let out = cast(&tools, "fail", json!([])).unwrap();

        assert!(
            out.starts_with("exit 3\n(only the last 16 KB)\n"),
            "{}",
            &out[..40]
        );
        assert!(out.ends_with("9999\n10000\nbroken\n"), "{out}");
        assert!(out.len() <= OUTPUT_LIMIT + 40);
    }

    /// A configured interactive bash, with no terminal, complains about job
    /// control: that is not the script's output.
    #[test]
    fn only_the_script_output_is_returned_through_an_interactive_shell() {
        let dir = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "say",
            "description = \"Say\"\nrun = \"run.sh\"\n",
            Some("#!/bin/sh\necho said\n"),
        );
        let rc = dir.path().join("rc");
        std::fs::write(&rc, "echo from rc\n").unwrap();
        let shell = ["bash", "--rcfile", rc.to_str().unwrap(), "-ic"].map(String::from);
        let tools = SpellTools::new(dir.path().to_path_buf(), shell.to_vec(), None);

        assert_eq!(cast(&tools, "say", json!([])).unwrap(), "exit 0\nsaid\n");
    }

    /// A spell runs in a session of its own, without Parolsh's terminal: an
    /// interactive bash in a background process group of that terminal
    /// would be stopped (SIGTTOU) as it takes it for its job control.
    #[test]
    fn a_spell_runs_in_its_own_session() {
        let dir = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "session",
            "description = \"Session\"\nrun = \"run.sh\"\n",
            Some("#!/bin/sh\nps -o sid=,pgid= -p $$\n"),
        );
        let tools = tools(dir.path(), None);

        let out = cast(&tools, "session", json!([])).unwrap();

        let ids: Vec<&str> = out.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(ids.len(), 2, "{out}");
        assert_eq!(ids[0], ids[1], "session id is not the group id: {out}");
    }

    #[test]
    fn a_cast_is_refused_when_it_cannot_run_as_written() {
        let dir = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "one",
            "description = \"One\"\nrun = \"run.sh\"\nargs = [{ name = \"file\" }]\n",
            Some("#!/bin/sh\n"),
        );
        spell(
            dir.path(),
            "plain",
            "description = \"Plain\"\nrun = \"plain.sh\"\n",
            None,
        );
        std::fs::write(dir.path().join("plain/plain.sh"), "#!/bin/sh\n").unwrap();
        spell(
            dir.path(),
            "manual",
            "description = \"M\"\ninstructions = \"do\"\n",
            None,
        );
        let tools = tools(dir.path(), None);
        let refused = |name, args| cast(&tools, name, args).unwrap_err().to_string();

        assert_eq!(
            refused("one", json!([])),
            "spell `one` takes 1 argument(s) (file), got 0"
        );
        assert!(
            refused("plain", json!([]))
                .ends_with("plain.sh is not executable: `chmod +x` it first")
        );
        assert!(refused("manual", json!([])).contains("has no script"));
        assert_eq!(
            refused("nope", json!([])),
            "no spell `nope`: list_spells lists them"
        );
        assert!(refused("one", json!([1])).contains("each argument is a string"));
    }

    #[test]
    fn get_spell_gives_the_agent_all_it_needs_to_follow_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "vs",
            r#"
description = "Install a mod"
run = "run.sh"
args = [{ name = "file", description = "The .zip" }]
env = { VS_HOST = "jg@host" }
instructions = "Check the version first.\n"
"#,
            Some("#!/bin/sh\nscp \"$1\" \"$VS_HOST:\"\n"),
        );
        spell(dir.path(), "broken", "run = 1\n", None);
        let here = Here::create(state.path(), state.path()).unwrap();
        let tools = tools(dir.path(), Some(here.path().to_path_buf()));

        let got = tools.call("get_spell", &json!({"name": "vs"})).unwrap();

        assert_eq!(
            got,
            format!(
                "vs <file>: Install a mod\n\
                 The user's commands run in {}: cast_spell runs the script there.\n\
                 \nArguments, in order:\n- file: The .zip\n\
                 \nEnvironment of the script:\n- VS_HOST=jg@host\n\
                 \nInstructions:\nCheck the version first.\n\
                 \nScript {}:\n```\n#!/bin/sh\nscp \"$1\" \"$VS_HOST:\"\n```\n",
                state.path().display(),
                dir.path().join("vs/run.sh").display()
            )
        );
        let listed = tools.call("list_spells", &json!({})).unwrap();
        assert!(
            listed.starts_with("vs <file>: Install a mod\n(cannot be used: invalid "),
            "{listed}"
        );
        let broken = tools
            .call("get_spell", &json!({"name": "broken"}))
            .unwrap_err();
        assert!(broken.to_string().starts_with("invalid "), "{broken}");
    }

    /// A script still running when `cast_spell` stops waiting goes on as a
    /// job: the agent follows it, and Parolsh sees it in its directory,
    /// running, then ended, once.
    #[test]
    fn a_long_spell_goes_on_as_a_job_that_parolsh_sees_end() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "slow",
            "description = \"Slow\"\nrun = \"run.sh\"\nargs = [{ name = \"what\" }]\n",
            Some("#!/bin/sh\necho started \"$1\"\nsleep 1\necho done\n"),
        );
        let here = Here::create(state.path(), state.path()).unwrap();
        let mut tools = tools(dir.path(), Some(here.path().to_path_buf()));
        tools.wait = Duration::from_millis(300);

        let out = cast(&tools, "slow", json!(["mod.zip"])).unwrap();

        assert!(
            out.starts_with("Still running after 0 s: it goes on in the background as job 1."),
            "{out}"
        );
        assert!(out.ends_with("Output so far:\nstarted mod.zip\n"), "{out}");
        let running = here.running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].label(), "spell job 1 · slow mod.zip");
        assert!(here.take_ended().is_empty());
        let job = tools.call("spell_job", &json!({"job": 1})).unwrap();
        assert!(
            job.starts_with("spell job 1 · slow mod.zip · running for "),
            "{job}"
        );

        eventually(|| here.running().is_empty());

        let ended = here.take_ended();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].exit, Some(0));
        assert!(here.take_ended().is_empty());
        let job = tools.call("spell_job", &json!({"job": 1})).unwrap();
        assert_eq!(
            job,
            "spell job 1 · slow mod.zip · ended: exit 0\nstarted mod.zip\ndone\n"
        );
        let all = tools.call("spell_job", &json!({})).unwrap();
        assert_eq!(all, "spell job 1 · slow mod.zip · ended: exit 0\n");
    }

    /// Parolsh forgets a job once it said it ended, but the server still
    /// shows it: the next job does not take its number.
    #[test]
    fn a_job_number_is_not_given_again_after_parolsh_said_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "slow",
            "description = \"Slow\"\nrun = \"run.sh\"\nargs = [{ name = \"what\" }]\n",
            Some("#!/bin/sh\necho \"$1\"\nsleep 1\n"),
        );
        let here = Here::create(state.path(), state.path()).unwrap();
        let mut tools = tools(dir.path(), Some(here.path().to_path_buf()));
        tools.wait = Duration::from_millis(300);
        cast(&tools, "slow", json!(["first"])).unwrap();
        eventually(|| here.running().is_empty());
        assert_eq!(here.take_ended().len(), 1);

        let out = cast(&tools, "slow", json!(["second"])).unwrap();

        assert!(out.contains("in the background as job 2."), "{out}");
        assert_eq!(here.running()[0].label(), "spell job 2 · slow second");
        let first = tools.call("spell_job", &json!({"job": 1})).unwrap();
        assert!(
            first.starts_with("spell job 1 · slow first · ended"),
            "{first}"
        );
        let second = tools.call("spell_job", &json!({"job": 2})).unwrap();
        assert!(
            second.starts_with("spell job 2 · slow second · running"),
            "{second}"
        );
    }

    /// `stop_spell` ends the script with what it started; Parolsh does the
    /// same when the conversation ends, and so does the server.
    #[test]
    fn a_job_stops_with_all_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        spell(
            dir.path(),
            "hang",
            "description = \"Hang\"\nrun = \"run.sh\"\n",
            Some("#!/bin/sh\necho waiting\nsleep 60 &\nsleep 60\n"),
        );
        let here = Here::create(state.path(), state.path()).unwrap();
        let mut tools = tools(dir.path(), Some(here.path().to_path_buf()));
        tools.wait = Duration::from_millis(200);
        let pgid = |id: u32| {
            here.jobs()
                .into_iter()
                .find(|job| job.id == id)
                .unwrap()
                .pgid
        };

        // By the agent.
        cast(&tools, "hang", json!([])).unwrap();
        let first = pgid(1);
        let stopped = tools.call("stop_spell", &json!({"job": 1})).unwrap();
        assert_eq!(stopped, "Job 1 stopped. exit 143\nwaiting\n");
        eventually(|| !shell::group_alive(first));
        let again = tools.call("stop_spell", &json!({"job": 1})).unwrap();
        assert!(
            again.starts_with("Job 1 had already ended. exit 143"),
            "{again}"
        );

        // By Parolsh, with the conversation: no notice of its end.
        cast(&tools, "hang", json!([])).unwrap();
        let second = pgid(2);
        here.stop_running();
        eventually(|| !shell::group_alive(second));
        here.take_ended();
        assert!(here.jobs().is_empty(), "{:?}", here.jobs());

        // By the server, when it ends.
        cast(&tools, "hang", json!([])).unwrap();
        let third = here.running()[0].pgid;
        drop(tools);
        eventually(|| !shell::group_alive(third));
    }

    #[test]
    fn the_here_directory_goes_with_parolsh() {
        let state = tempfile::tempdir().unwrap();
        let here = Here::create(&state.path().join("run"), state.path()).unwrap();
        let path = here.path().to_path_buf();
        assert_eq!(read_here(&path), Some(state.path().to_path_buf()));

        drop(here);

        assert!(!path.exists());
        assert_eq!(read_here(&path), None);
    }

    #[test]
    fn the_server_answers_mcp_with_its_tools() {
        let dir = tempfile::tempdir().unwrap();
        let tools = tools(dir.path(), None);
        let reply = mcp::handle(
            &tools,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        )
        .unwrap();
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "list_spells",
                "get_spell",
                "cast_spell",
                "spell_job",
                "stop_spell"
            ]
        );
        assert_eq!(
            tools.call("list_spells", &json!({})).unwrap(),
            "No spells yet.\n"
        );
        assert_eq!(
            tools.call("spell_job", &json!({})).unwrap(),
            "No spells in the background.\n"
        );
    }
}
