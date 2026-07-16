//! Command-line entry point for importing and querying local agent activity.

use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use autophagy_adapter_claude_code::{
    ClaudeImportOptions, ClaudeImportSummary, default_projects_root, import_claude_code,
};
use autophagy_adapter_codex::{
    CodexImportOptions, CodexImportSummary, default_sessions_root, import_codex,
};
use autophagy_core::{ImportOptions, ImportSummary, import_jsonl};
use autophagy_store::{EventStore, SearchHit, SessionSummary, StoreError};
use clap::{Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "autophagy",
    version,
    about = "The self-improvement layer for local coding agents",
    arg_required_else_help = true
)]
struct Cli {
    /// Local database path. Defaults to the platform-local application data directory.
    #[arg(long, global = true, env = "AUTOPHAGY_DB", value_name = "PATH")]
    database: Option<PathBuf>,

    /// Output format for command results.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    output: OutputFormat,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ImportAdapter {
    GenericJsonl,
    ClaudeCode,
    Codex,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Import normalized AEP JSONL or native agent history.
    Import {
        /// Input file/root. `-` means stdin for generic JSONL or the adapter's default history root.
        #[arg(default_value = "-", value_name = "PATH")]
        input: PathBuf,

        /// Source format to discover and normalize.
        #[arg(long, value_enum, default_value_t = ImportAdapter::GenericJsonl)]
        adapter: ImportAdapter,

        /// Stable source identity. Defaults to the canonical input path or `stdin`.
        #[arg(long, value_name = "KEY")]
        instance_key: Option<String>,

        /// Optional user-facing label for this source.
        #[arg(long, value_name = "NAME")]
        display_name: Option<String>,

        /// Include only events with this exact project path. Repeatable.
        #[arg(long = "project", value_name = "PATH")]
        projects: Vec<String>,

        /// Include Claude Code `agent-*.jsonl` subagent transcripts.
        #[arg(long)]
        include_subagents: bool,

        /// Persist native-adapter prompt, response, and tool-result text in event metadata.
        #[arg(long)]
        include_content: bool,

        /// Index tool input after confirming the source has already been redacted.
        #[arg(long)]
        index_tool_input: bool,

        /// Index an already-redacted event metadata key. Repeatable.
        #[arg(long = "index-metadata", value_name = "KEY")]
        index_metadata: Vec<String>,

        /// Parse and filter without creating or changing the database.
        #[arg(long)]
        dry_run: bool,

        /// Maximum line diagnostics retained in the result.
        #[arg(long, default_value_t = 100, value_name = "COUNT")]
        max_diagnostics: usize,
    },

    /// List recently active imported sessions.
    Sessions {
        /// Maximum number of sessions to return.
        #[arg(long, default_value_t = 50, value_name = "COUNT")]
        limit: u32,
    },

    /// Search the redaction-approved FTS5 event projection.
    Search {
        /// FTS5 query expression.
        query: String,

        /// Maximum number of matches to return.
        #[arg(long, default_value_t = 20, value_name = "COUNT")]
        limit: u32,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "command", content = "result", rename_all = "snake_case")]
enum CommandReport {
    Import(ImportReport),
    Sessions(Vec<SessionSummary>),
    Search(Vec<SearchHit>),
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ImportReport {
    Generic(ImportSummary),
    ClaudeCode(ClaudeImportSummary),
    Codex(CodexImportSummary),
}

impl ImportReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Generic(summary) => summary.has_issues(),
            Self::ClaudeCode(summary) => summary.has_issues(),
            Self::Codex(summary) => summary.has_issues(),
        }
    }
}

impl CommandReport {
    const fn has_issues(&self) -> bool {
        match self {
            Self::Import(summary) => summary.has_issues(),
            Self::Sessions(_) | Self::Search(_) => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("I/O operation failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Import(#[from] autophagy_core::ImportError),
    #[error(transparent)]
    ClaudeImport(#[from] autophagy_adapter_claude_code::ClaudeImportError),
    #[error(transparent)]
    ClaudeDiscovery(#[from] autophagy_adapter_claude_code::DiscoveryError),
    #[error(transparent)]
    CodexImport(#[from] autophagy_adapter_codex::CodexImportError),
    #[error(transparent)]
    CodexDiscovery(#[from] autophagy_adapter_codex::CodexDiscoveryError),
    #[error("could not serialize command output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("could not determine the platform-local application data directory")]
    DataDirectoryUnavailable,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = cli.output;
    match execute(cli).and_then(|report| {
        let has_issues = report.has_issues();
        write_report(io::stdout().lock(), output, &report)?;
        Ok(has_issues)
    }) {
        Ok(true) => ExitCode::from(2),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<CommandReport, CliError> {
    match cli.command {
        Commands::Import {
            input,
            adapter,
            instance_key,
            display_name,
            projects,
            include_subagents,
            include_content,
            index_tool_input,
            index_metadata,
            dry_run,
            max_diagnostics,
        } => match adapter {
            ImportAdapter::GenericJsonl => {
                let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                let mut options = ImportOptions::new(instance_key);
                options.display_name = display_name;
                options.projects = projects;
                options.index_tool_input = index_tool_input;
                options.index_metadata = index_metadata;
                options.dry_run = dry_run;
                options.max_diagnostics = max_diagnostics;
                let reader = open_input(&input)?;
                let summary = if dry_run {
                    import_jsonl(reader, None, &options)?
                } else {
                    let database = resolve_database_path(cli.database)?;
                    let mut store = open_store(&database)?;
                    import_jsonl(reader, Some(&mut store), &options)?
                };
                Ok(CommandReport::Import(ImportReport::Generic(summary)))
            }
            ImportAdapter::ClaudeCode => {
                let input = if input == Path::new("-") {
                    default_projects_root()?
                } else {
                    input
                };
                let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                let mut options = ClaudeImportOptions::new(input, instance_key);
                options.display_name = display_name;
                options.projects = projects;
                options.include_subagents = include_subagents;
                options.include_content = include_content;
                options.index_tool_input = index_tool_input;
                options.index_metadata = index_metadata;
                options.dry_run = dry_run;
                options.max_diagnostics = max_diagnostics;
                let summary = if dry_run {
                    import_claude_code(None, &options)?
                } else {
                    let database = resolve_database_path(cli.database)?;
                    let mut store = open_store(&database)?;
                    import_claude_code(Some(&mut store), &options)?
                };
                Ok(CommandReport::Import(ImportReport::ClaudeCode(summary)))
            }
            ImportAdapter::Codex => {
                let input = if input == Path::new("-") {
                    default_sessions_root()?
                } else {
                    input
                };
                let instance_key = instance_key.unwrap_or(derive_instance_key(&input)?);
                let mut options = CodexImportOptions::new(input, instance_key);
                options.display_name = display_name;
                options.projects = projects;
                options.include_content = include_content;
                options.index_tool_input = index_tool_input;
                options.index_metadata = index_metadata;
                options.dry_run = dry_run;
                options.max_diagnostics = max_diagnostics;
                let summary = if dry_run {
                    import_codex(None, &options)?
                } else {
                    let database = resolve_database_path(cli.database)?;
                    let mut store = open_store(&database)?;
                    import_codex(Some(&mut store), &options)?
                };
                Ok(CommandReport::Import(ImportReport::Codex(summary)))
            }
        },
        Commands::Sessions { limit } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            Ok(CommandReport::Sessions(store.list_sessions(limit)?))
        }
        Commands::Search { query, limit } => {
            let database = resolve_database_path(cli.database)?;
            let store = open_store(&database)?;
            Ok(CommandReport::Search(store.search(&query, limit)?))
        }
    }
}

fn resolve_database_path(path: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = path {
        return Ok(path);
    }
    let project = ProjectDirs::from("sh", "autophagy", "Autophagy")
        .ok_or(CliError::DataDirectoryUnavailable)?;
    Ok(project.data_local_dir().join("autophagy.db"))
}

fn open_store(path: &Path) -> Result<EventStore, CliError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    Ok(EventStore::open(path)?)
}

fn open_input(path: &Path) -> Result<Box<dyn BufRead>, CliError> {
    if path == Path::new("-") {
        Ok(Box::new(BufReader::new(io::stdin())))
    } else {
        Ok(Box::new(BufReader::new(File::open(path)?)))
    }
}

fn derive_instance_key(input: &Path) -> Result<String, CliError> {
    if input == Path::new("-") {
        Ok("stdin".to_owned())
    } else {
        Ok(fs::canonicalize(input)?.to_string_lossy().into_owned())
    }
}

fn write_report(
    mut writer: impl Write,
    format: OutputFormat,
    report: &CommandReport,
) -> Result<(), CliError> {
    match format {
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut writer, report)?;
            writeln!(writer)?;
        }
        OutputFormat::Text => match report {
            CommandReport::Import(summary) => match summary {
                ImportReport::Generic(summary) => write_import_summary(&mut writer, summary)?,
                ImportReport::ClaudeCode(summary) => {
                    write_claude_import_summary(&mut writer, summary)?;
                }
                ImportReport::Codex(summary) => write_codex_import_summary(&mut writer, summary)?,
            },
            CommandReport::Sessions(sessions) => {
                writeln!(writer, "SESSION\tSOURCE\tEVENTS\tLAST EVENT\tPROJECT")?;
                for session in sessions {
                    writeln!(
                        writer,
                        "{}\t{}\t{}\t{}\t{}",
                        session.session_id,
                        session.adapter,
                        session.event_count,
                        session.last_event_at,
                        session.project_path.as_deref().unwrap_or("-")
                    )?;
                }
            }
            CommandReport::Search(hits) => {
                for hit in hits {
                    writeln!(writer, "{}\t{}", hit.event_id, hit.snippet)?;
                }
            }
        },
    }
    Ok(())
}

fn write_codex_import_summary(
    writer: &mut impl Write,
    summary: &CodexImportSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{} files · {} records · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} rejected{}",
        summary.discovery.files.len(),
        summary.records_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for file in &summary.discovery.files {
        writeln!(writer, "{}\t{} bytes", file.relative_path, file.size_bytes)?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{}:{} [{}] {}",
            diagnostic.file, diagnostic.line, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_claude_import_summary(
    writer: &mut impl Write,
    summary: &ClaudeImportSummary,
) -> io::Result<()> {
    writeln!(
        writer,
        "{} files · {} records · {} events · {} inserted · {} duplicates · {} conflicts · {} unsupported · {} rejected{}",
        summary.discovery.files.len(),
        summary.records_seen,
        summary.events_emitted,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.unsupported,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for file in &summary.discovery.files {
        writeln!(writer, "{}\t{} bytes", file.relative_path, file.size_bytes)?;
    }
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "{}:{} [{}] {}",
            diagnostic.file, diagnostic.line, diagnostic.code, diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}

fn write_import_summary(writer: &mut impl Write, summary: &ImportSummary) -> io::Result<()> {
    writeln!(
        writer,
        "{} lines · {} events · {} inserted · {} duplicates · {} conflicts · {} skipped · {} rejected{}",
        summary.lines_read,
        summary.events_seen,
        summary.inserted,
        summary.duplicates,
        summary.conflicts,
        summary.skipped,
        summary.rejected,
        if summary.dry_run { " · dry run" } else { "" }
    )?;
    for diagnostic in &summary.diagnostics {
        writeln!(
            writer,
            "line {} [{}] {}",
            diagnostic.line,
            diagnostic.code.as_str(),
            diagnostic.message
        )?;
    }
    if summary.diagnostics_suppressed > 0 {
        writeln!(
            writer,
            "{} additional diagnostics suppressed",
            summary.diagnostics_suppressed
        )?;
    }
    Ok(())
}
