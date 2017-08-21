#![recursion_limit = "1024"]

#[macro_use] extern crate clap;
extern crate serde;
extern crate serde_json;
#[macro_use] extern crate error_chain;
extern crate flate2;
extern crate tar;
extern crate rustc_perf_collector;
extern crate env_logger;
extern crate tempdir;
#[macro_use] extern crate log;
extern crate reqwest;
extern crate chrono;
extern crate rust_sysroot;

mod errors {
    // Create the Error, ErrorKind, ResultExt, and Result types
    error_chain! {
        links {
            RustSysroot(::rust_sysroot::errors::Error, ::rust_sysroot::errors::ErrorKind);
        }

        foreign_links {
            Reqwest(::reqwest::Error);
            Serde(::serde_json::Error);
            Chrono(::chrono::ParseError);
            Io(::std::io::Error);
        }
    }
}

use errors::*;

quick_main!(run);

use std::fs;
use std::str;
use std::path::{Path, PathBuf};
use std::io::{stdout, stderr, Write};
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use rustc_perf_collector::{Commit, CommitData};
use rust_sysroot::git::Commit as GitCommit;
use rust_sysroot::sysroot::Sysroot;

mod git;
mod execute;
mod outrepo;
mod time_passes;

use execute::Benchmark;

fn bench_commit(
    commit: &GitCommit,
    repo: Option<&outrepo::Repo>,
    sysroot: Sysroot,
    benchmarks: &[Benchmark]
) -> CommitData {
    info!("benchmarking commit {} ({}) for triple {}", commit.sha, commit.date, sysroot.triple);

    let existing_data = repo.and_then(|r| r.load_commit_data(&commit, &sysroot.triple).ok());

    let results: BTreeMap<_, _> = benchmarks.iter().map(|benchmark| {
        if let Some(ref data) = existing_data {
            if let Some(result) = data.benchmarks.get(&benchmark.name) {
                return (benchmark.name.clone(), result.clone());
            }
        }

        let result = benchmark.run(&sysroot);

        if result.is_err() {
            info!("failure to benchmark {}, recorded: {:?}", benchmark.name, result);
        }

        (benchmark.name.clone(), result.map_err(|e| format!("{:?}", e)))
    }).collect();

    CommitData {
        commit: Commit {
            sha: commit.sha.clone(),
            date: commit.date,
        },
        triple: sysroot.triple.clone(),
        benchmarks: results
    }
}

fn get_benchmarks(benchmark_dir: &Path, filter: Option<&str>) -> Result<Vec<Benchmark>> {
    let mut benchmarks = Vec::new();
    for entry in fs::read_dir(benchmark_dir).chain_err(|| "failed to list benchmarks")? {
        let entry = entry?;
        let path = entry.path();
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(e) => bail!("non-utf8 benchmark name: {:?}", e)
        };

        if path.ends_with(".git") || path.ends_with("scripts") || !entry.file_type()?.is_dir() {
            info!("benchmark {} - ignored", name);
            continue;
        }

        if let Some(filter) = filter {
            if !name.contains(filter) {
                info!("benchmark {} - filtered", name);
                continue;
            }
        }

        info!("benchmark {} - REGISTERED", name);
        benchmarks.push(Benchmark {
            path: path,
            name: name
        });
    }
    Ok(benchmarks)
}

fn process_commit(repo: &outrepo::Repo, commit: &GitCommit, benchmarks: &[Benchmark], preserve_sysroot: bool) -> Result<()> {
    let sysroot = Sysroot::install(commit, "x86_64-unknown-linux-gnu", preserve_sysroot, false)?;
    repo.success(&bench_commit(commit, Some(repo), sysroot, benchmarks))
}


fn process_retries(commits: &[GitCommit], repo: &mut outrepo::Repo, benchmarks: &[Benchmark], preserve_sysroot: bool)
                   -> Result<()>
{
    while let Some(retry) = repo.next_retry() {
        info!("retrying {}", retry);
        let commit = commits.iter().find(|commit| commit.sha == retry).unwrap();
        process_commit(repo, commit, benchmarks, preserve_sysroot)?;
    }
    Ok(())
}

fn process_commits(commits: &[GitCommit], repo: &outrepo::Repo, benchmarks: &[Benchmark], preserve_sysroot: bool)
                   -> Result<()>
{
    println!("processing commits");
    if !commits.is_empty() {
        let to_process = repo.find_missing_commits(commits, benchmarks, "x86_64-unknown-linux-gnu")?;
        // take 3 from the end -- this means that for each bors commit (which takes ~3 hours) we
        // test 3, which should allow us to eventually test all commits, but also keep up with the
        // latest rustc
        for commit in to_process.iter().rev().take(3) {
            process_commit(repo, &commit, &benchmarks, preserve_sysroot)?;
        }
    } else {
        info!("Nothing to do; no commits.");
    }
    Ok(())
}

fn run() -> Result<i32> {
    env_logger::init().expect("logger initialization successful");
    git::fetch_rust(Path::new("rust.git"))?;

    let matches = clap_app!(rustc_perf_collector =>
       (version: "0.1")
       (author: "The Rust Compiler Team")
       (about: "Collects Rust performance data")
       (@arg benchmarks_dir: --benchmarks-dir +required +takes_value "Sets the directory benchmarks are found in")
       (@arg filter: --filter +takes_value "Run only benchmarks that contain this")
       (@arg preserve_sysroots: -p --preserve "Don't delete sysroots after running.")
       (@subcommand process =>
           (about: "syncs to git and collects performance data for all versions")
           (@arg OUTPUT_REPOSITORY: +required +takes_value "Repository to output to")
       )
       (@subcommand bench_commit =>
           (about: "benchmark a bors merge from AWS and output data to stdout")
           (@arg COMMIT: +required +takes_value "Commit hash to bench")
       )
       (@subcommand bench_local =>
           (about: "benchmark a bors merge from AWS and output data to stdout")
           (@arg COMMIT: --commit +required +takes_value "Commit hash to associate benchmark results with")
           (@arg DATE: --date +required +takes_value "Date to associate benchmark result with, YYYY-MM-DDTHH:MM:SS format.")
           (@arg RUSTC: +required +takes_value "the path to the local rustc to benchmark")
       )
       (@subcommand remove_errs =>
           (about: "remove errored data")
           (@arg OUTPUT_REPOSITORY: +required +takes_value "Repository to output to")
       )
       (@subcommand remove_benchmark =>
           (about: "remove data for a benchmark")
           (@arg BENCHMARK: --benchmark +required +takes_value "benchmark name to remove data for")
           (@arg OUTPUT_REPOSITORY: +required +takes_value "Repository to output to")
       )
    ).get_matches();
    let benchmark_dir = PathBuf::from(matches.value_of_os("benchmarks_dir").unwrap());
    let filter = matches.value_of("filter");
    let benchmarks = get_benchmarks(&benchmark_dir, filter)?;
    let preserve_sysroots = matches.is_present("preserve_sysroots");

    let commits = rust_sysroot::get_commits()?;

    match matches.subcommand() {
        ("process", Some(sub_m)) => {
            let out_repo = PathBuf::from(sub_m.value_of_os("OUTPUT_REPOSITORY").unwrap());
            let mut out_repo = outrepo::Repo::open(out_repo)?;
            process_retries(&commits, &mut out_repo, &benchmarks, preserve_sysroots)?;
            process_commits(&commits, &out_repo, &benchmarks, preserve_sysroots)?;
            Ok(0)
        }
        ("bench_commit", Some(sub_m)) => {
            let commit = sub_m.value_of("COMMIT").unwrap();
            let commit = commits.iter().find(|c| c.sha == commit).unwrap();
            let sysroot = Sysroot::install(&commit, "x86_64-unknown-linux-gnu", preserve_sysroots, false)?;
            let result = bench_commit(&commit, None, sysroot, &benchmarks);
            serde_json::to_writer(&mut stdout(), &result)?;
            Ok(0)
        }
        ("bench_local", Some(sub_m)) => {
            let commit = sub_m.value_of("COMMIT").unwrap();
            let date = sub_m.value_of("DATE").unwrap();
            let rustc = sub_m.value_of("RUSTC").unwrap();
            let commit = GitCommit { sha: commit.to_string(), date: DateTime::parse_from_rfc3339(date)?.with_timezone(&Utc), summary: String::new() };
            let sysroot = Sysroot::with_local_rustc(&commit, rustc, "x86_64-unknown-linux-gnu", preserve_sysroots, false)?;
            let result = bench_commit(&commit, None, sysroot, &benchmarks);
            serde_json::to_writer(&mut stdout(), &result)?;
            Ok(0)
        }
        ("remove_errs", Some(sub_m)) => {
            let out_repo = PathBuf::from(sub_m.value_of_os("OUTPUT_REPOSITORY").unwrap());
            let out_repo = outrepo::Repo::open(out_repo)?;
            for commit in &commits {
                if let Ok(mut data) = out_repo.load_commit_data(&commit, "x86_64-unknown-linux-gnu") {
                    let benchmarks = data.benchmarks.into_iter().filter(|&(_, ref v)| v.is_ok()).collect();
                    data.benchmarks = benchmarks;
                    out_repo.add_commit_data(&data)?;
                }
            }
            Ok(0)
        }
        ("remove_benchmark", Some(sub_m)) => {
            let out_repo = PathBuf::from(sub_m.value_of_os("OUTPUT_REPOSITORY").unwrap());
            let benchmark = sub_m.value_of("BENCHMARK").unwrap();
            let out_repo = outrepo::Repo::open(out_repo)?;
            for commit in &commits {
                if let Ok(mut data) = out_repo.load_commit_data(&commit, "x86_64-unknown-linux-gnu") {
                    if data.benchmarks.remove(&*benchmark).is_none() {
                        warn!("could not remove {} from {}", benchmark, commit.sha);
                    }
                    out_repo.add_commit_data(&data)?;
                }
            }
            Ok(0)
        }
        _ => {
            let _ = writeln!(stderr(), "{}", matches.usage());
            Ok(2)
        }
    }
}
