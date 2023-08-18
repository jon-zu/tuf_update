use std::{
    collections::HashSet,
    fs::File,
    path::{PathBuf, Path},
    time::{Duration, Instant},
};

use derive_builder::Builder;
use snafu::GenerateImplicitData;
use tough::{schema::Target, Repository, RepositoryLoader, TargetName};
use url::Url;

use crate::manifest::Manifest;

pub type UpdateError = anyhow::Error;

// TODO: might aswell use a better error type
fn create_update_error(name: &TargetName, err: tough::error::Error) -> UpdateError {
    anyhow::Error::from(err).context(format!("failed to update target: {}", name.resolved()))
}

#[derive(Debug)]
pub enum UpdateProgress {
    StartFileDownload(TargetName),
    UpdateFileProgress(u64, u64),
    FinishFileDownload,
    FinishUpdate,
}

pub trait ProgressWatcher: std::fmt::Debug {
    fn update_progress(&self, progress: UpdateProgress);
}

#[derive(Debug, Default)]
pub struct UpdateReport {
    pub updated_files: usize,
    pub deleted_files: usize,
    pub update_time: Duration,
}

#[derive(Debug)]
pub enum UpdateResult {
    AlreadyUpdated,
    IncompleteUpdate {
        errs: Vec<UpdateError>,
        report: UpdateReport,
    },
    CompleteUpdate(UpdateReport),
}

#[derive(Builder, Debug)]
#[builder(setter(into), pattern = "owned")]
pub struct Updater {
    repo: Repository,
    manifest_file: PathBuf,
    dist_dir: PathBuf,
    #[builder(default)]
    watcher: Option<Box<dyn ProgressWatcher>>,
    safe_delete_exe_target: String
}

impl Updater {
    pub fn load_basic_http_repo(base_url: &str, tuf_dir: impl AsRef<Path>) -> anyhow::Result<Repository> {
        let base_url = Url::parse(base_url)?;
        let tuf_dir = tuf_dir.as_ref().to_path_buf();
        Ok(RepositoryLoader::new(
            // Root json in the tuf directory
            File::open(tuf_dir.join("root.json"))?,
            base_url.join("/metadata")?,
            base_url.join("/targets")?,
        )
        .datastore(tuf_dir.join("dist"))
        .load()?)
    }

    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    fn update_progress(&self, progress: UpdateProgress) {
        if let Some(ref watcher) = self.watcher {
            watcher.update_progress(progress);
        }
    }

    fn update_target(
        &self,
        manifest: &mut Manifest,
        (name, target): (&TargetName, &Target),
    ) -> Result<(), UpdateError> {
        if manifest.is_target_updated(name, target.length, &target.hashes.sha256) {
            return Ok(());
        }

        self.update_progress(UpdateProgress::StartFileDownload(name.clone()));

        // TODO: download
        self.update_progress(UpdateProgress::UpdateFileProgress(50, 100));

        // Determine if self delete is required 
        if self.safe_delete_exe_target == name.resolved() {
            self_replace::self_delete().expect("Self delete");
            println!("Safe delete: {}", name.resolved());
        }


        self.repo
            .save_target(name, &self.dist_dir, tough::Prefix::None)
            .map_err(|err| create_update_error(name, err))?;

        self.update_progress(UpdateProgress::FinishFileDownload);

        manifest.set_target(name, target.length, &target.hashes.sha256);
        Ok(())
    }

    fn update_all_targets(&mut self, manifest: &mut Manifest) -> (usize, Vec<UpdateError>) {
        let targets = &self.repo.targets().signed;

        let mut errs = vec![];
        for (name, target) in targets.targets_iter() {
            if let Err(err) = self.update_target(manifest, (name, target)) {
                errs.push(err);
            }
        }
        let updated_files = targets.targets.len() - errs.len();
        self.update_progress(UpdateProgress::FinishUpdate);

        (updated_files, errs)
    }

    fn delete_target(&self, name: &TargetName) -> anyhow::Result<()> {
        let path = self.dist_dir.join(name.resolved());
        if path.exists() {
            std::fs::remove_file(&path).map_err(|err| {
                create_update_error(
                    name,
                    tough::error::Error::RemoveTarget {
                        path,
                        source: err,
                        backtrace: snafu::Backtrace::generate(),
                    },
                )
            })?;
        }
        Ok(())
    }

    fn delete_removed_targets(&self, manifest: &mut Manifest) -> (usize, Vec<anyhow::Error>) {
        let targets = self.repo.targets().signed.targets_iter();
        let target_names = targets.map(|(name, _)| name).collect::<HashSet<_>>();
        let mut deleted_files = 0;

        let mut errs = vec![];

        manifest.retain_targets(|name| {
            if target_names.contains(name) {
                return true;
            }

            if let Err(err) = self.delete_target(name) {
                errs.push(err);
            } else {
                deleted_files += 1;
            }
            false
        });

        (deleted_files, errs)
    }

    pub fn update(&mut self) -> anyhow::Result<UpdateResult> {
        let start = Instant::now();
        let snapshot_version = self.repo.snapshot().signed.version;

        // Read manifest or create a new one
        let mut manifest = Manifest::load_or_new(&self.manifest_file)?;

        if manifest.is_updated(snapshot_version) {
            return Ok(UpdateResult::AlreadyUpdated);
        }

        let (updated_files, update_errs) = self.update_all_targets(&mut manifest);
        let (deleted_files, deleted_errs) = self.delete_removed_targets(&mut manifest);

        let mut errs: Vec<anyhow::Error> = update_errs;
        errs.extend(deleted_errs);
        manifest.set_update_complete_result(errs.is_empty());

        manifest.update_version(snapshot_version);
        // Save Manifest
        manifest.save(&self.manifest_file)?;

        let report = UpdateReport {
            updated_files,
            deleted_files,
            update_time: start.elapsed(),
        };

        Ok(if errs.is_empty() {
            UpdateResult::CompleteUpdate(report)
        } else {
            UpdateResult::IncompleteUpdate { errs, report }
        })
    }
}
