//! Cleanup apply/revert orchestration and recovery under the library coordinator.
//! Preparation produces typed file mutations; each effect is journalled before
//! execution and committed with its catalog/history update before publication.
use super::*;

mod prepare;
use prepare::{cleanup_mutation_failure_reason, cleanup_path_depth, cleanup_revert_failure_reason};

#[derive(Debug)]
struct PreparedCleanupMutation {
    track_id: i64,
    kind: PreparedCleanupKind,
    mutation: LibraryFileMutation,
    append: CleanupBatchAppend,
}

#[derive(Debug)]
enum PreparedCleanupKind {
    Rename { target_name: String },
    Tag,
    FolderRename { target_name: String },
}

#[derive(Debug)]
struct AppliedCleanupMutation {
    affected_tracks: u64,
    batch_id: i64,
}

#[derive(Debug)]
struct PreparedCleanupRevert {
    track_id: i64,
    kind: PreparedCleanupRevertKind,
    mutation: LibraryFileMutation,
    revert: CleanupRevertMutation,
}

#[derive(Debug)]
enum PreparedCleanupRevertKind {
    Rename { original_name: String },
    Tag,
    FolderRename { original_name: String },
}

#[derive(Debug)]
enum CleanupPreparationError {
    Skip(CleanupSkip),
    Fatal(LibraryCoordinatorError),
}

impl From<CleanupSkip> for CleanupPreparationError {
    fn from(skip: CleanupSkip) -> Self {
        Self::Skip(skip)
    }
}

impl LibraryCoordinator {
    pub(super) async fn apply_cleanup_once(
        &self,
        batch_id: Option<i64>,
        scope_label: String,
        operations: Vec<CleanupApplyOperation>,
    ) -> Result<CleanupApplyResult, LibraryCoordinatorError> {
        if !(1..=MAX_CLEANUP_APPLY_OPERATIONS).contains(&operations.len()) {
            return Err(LibraryCoordinatorError::InvalidCleanupBatchSize);
        }
        if scope_label.chars().count() > MAX_CLEANUP_SCOPE_LABEL_CHARS {
            return Err(LibraryCoordinatorError::InvalidCleanupScopeLabel);
        }
        if let Some(batch_id) = batch_id {
            if batch_id <= 0 {
                return Err(LibraryCoordinatorError::CleanupBatchNotFound);
            }
            let batch = self
                .repository
                .cleanup_batch(batch_id)
                .await
                .map_err(|source| dependency("load the cleanup append target", source))?
                .ok_or(LibraryCoordinatorError::CleanupBatchNotFound)?;
            if batch.reverted_at_unix_seconds.is_some() {
                return Err(LibraryCoordinatorError::CleanupBatchReverted);
            }
        }

        let (mut regular, mut folders): (Vec<_>, Vec<_>) = operations
            .into_iter()
            .partition(|operation| operation.kind != CleanupOperationKind::FolderRename);
        folders.sort_by(|left, right| {
            cleanup_path_depth(&right.path).cmp(&cleanup_path_depth(&left.path))
        });
        regular.extend(folders);

        let mut current_batch_id = batch_id;
        let mut applied = 0_usize;
        let mut catalog_changed = false;
        let mut skipped = Vec::new();
        for operation in regular {
            let prepared = match self
                .prepare_cleanup_mutation(current_batch_id, &scope_label, operation)
                .await
            {
                Ok(prepared) => prepared,
                Err(CleanupPreparationError::Skip(skip)) => {
                    skipped.push(skip);
                    continue;
                }
                Err(CleanupPreparationError::Fatal(error)) => return Err(error),
            };
            match self.apply_cleanup_mutation(&prepared).await {
                Ok(commit) => {
                    current_batch_id = Some(commit.batch_id);
                    applied += 1;
                    catalog_changed |= commit.affected_tracks > 0;
                }
                Err(LibraryCoordinatorError::Mutation(failure)) if !failure.requires_recovery() => {
                    skipped.push(CleanupSkip {
                        track_id: prepared.track_id,
                        reason: cleanup_mutation_failure_reason(&prepared.kind, &failure),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        if catalog_changed {
            self.publish_current_catalog().await?;
        }
        Ok(CleanupApplyResult {
            batch_id: current_batch_id,
            applied,
            skipped,
        })
    }

    pub(super) async fn revert_cleanup_batch_once(
        &self,
        batch_id: i64,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        if batch_id <= 0 {
            return Err(LibraryCoordinatorError::CleanupBatchNotFound);
        }
        let batch = self
            .repository
            .cleanup_batch(batch_id)
            .await
            .map_err(|source| dependency("load the cleanup batch for reverting", source))?
            .ok_or(LibraryCoordinatorError::CleanupBatchNotFound)?;
        if batch.reverted_at_unix_seconds.is_some() {
            return Err(LibraryCoordinatorError::CleanupBatchReverted);
        }

        let operation = RecoveryOperation::parse("revert_batch").map_err(|source| {
            dependency("validate a cleanup batch revert journal", Box::new(source))
        })?;
        let draft = RecoveryJournalDraft::new(
            RecoveryDomain::Cleanup,
            operation,
            json!({"batch_id": batch_id}),
        )
        .map_err(|source| {
            dependency("validate a cleanup batch revert journal", Box::new(source))
        })?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup batch revert journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup batch revert journal", source))?,
        )?;
        let result = self
            .revert_cleanup_items(Some(batch_id), batch.items)
            .await?;
        self.repository
            .finish_cleanup_batch_revert(
                &applying.id,
                batch_id,
                result.reverted,
                result.skipped.len(),
            )
            .await
            .map_err(|source| dependency("finish a cleanup batch revert", source))?;
        Ok(result)
    }

    pub(super) async fn revert_cleanup_journal_once(
        &self,
        items: Vec<Map<String, Value>>,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        if !(1..=MAX_CLEANUP_REVERT_ITEMS).contains(&items.len()) {
            return Err(LibraryCoordinatorError::InvalidCleanupRevertSize);
        }
        self.revert_cleanup_items(None, items).await
    }

    async fn revert_cleanup_items(
        &self,
        batch_id: Option<i64>,
        items: Vec<Map<String, Value>>,
    ) -> Result<CleanupRevertResult, LibraryCoordinatorError> {
        let mut reverted = 0_usize;
        let mut catalog_changed = false;
        let mut skipped = Vec::new();
        for (item_index, item) in items.into_iter().enumerate().rev() {
            let prepared = match self
                .prepare_cleanup_revert(batch_id, item_index, &item)
                .await
            {
                Ok(prepared) => prepared,
                Err(CleanupPreparationError::Skip(skip)) => {
                    skipped.push(skip);
                    continue;
                }
                Err(CleanupPreparationError::Fatal(error)) => return Err(error),
            };
            match self.apply_cleanup_revert_mutation(&prepared).await {
                Ok(affected_tracks) => {
                    reverted += 1;
                    catalog_changed |= affected_tracks > 0;
                }
                Err(LibraryCoordinatorError::Mutation(failure)) if !failure.requires_recovery() => {
                    skipped.push(CleanupSkip {
                        track_id: prepared.track_id,
                        reason: cleanup_revert_failure_reason(&prepared.kind, &failure),
                    });
                }
                Err(error) => return Err(error),
            }
        }
        if catalog_changed {
            self.publish_current_catalog().await?;
        }
        Ok(CleanupRevertResult { reverted, skipped })
    }

    async fn apply_cleanup_mutation(
        &self,
        prepared: &PreparedCleanupMutation,
    ) -> Result<AppliedCleanupMutation, LibraryCoordinatorError> {
        let operation = prepared
            .mutation
            .operation()
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let plan = prepared
            .append
            .journal_plan(&prepared.mutation)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let draft = RecoveryJournalDraft::new(RecoveryDomain::Cleanup, operation, plan).map_err(
            |source| dependency("validate a cleanup mutation journal", Box::new(source)),
        )?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup mutation journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup mutation journal", source))?,
        )?;
        let outcome = self
            .apply_cleanup_file_effect(
                &applying,
                &prepared.mutation,
                "record a failed cleanup mutation",
            )
            .await?;
        let commit = self
            .repository
            .commit_cleanup_mutation(&applying.id, &prepared.mutation, outcome, &prepared.append)
            .await
            .map_err(|source| dependency("commit a cleanup mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(AppliedCleanupMutation {
            affected_tracks: commit.affected_tracks,
            batch_id: commit.batch_id,
        })
    }

    async fn apply_cleanup_revert_mutation(
        &self,
        prepared: &PreparedCleanupRevert,
    ) -> Result<u64, LibraryCoordinatorError> {
        let operation = prepared
            .mutation
            .operation()
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let plan = prepared
            .revert
            .journal_plan(&prepared.mutation)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let draft = RecoveryJournalDraft::new(RecoveryDomain::Cleanup, operation, plan).map_err(
            |source| {
                dependency(
                    "validate a cleanup revert mutation journal",
                    Box::new(source),
                )
            },
        )?;
        let planned = self
            .repository
            .create_recovery_journal(draft)
            .await
            .map_err(|source| dependency("create a cleanup revert mutation journal", source))?;
        let applying = transition_applied(
            self.repository
                .transition_recovery_journal(
                    &planned.id,
                    RecoveryState::Planned,
                    RecoveryState::Applying,
                    json!({}),
                )
                .await
                .map_err(|source| dependency("start a cleanup revert mutation journal", source))?,
        )?;
        let outcome = self
            .apply_cleanup_file_effect(
                &applying,
                &prepared.mutation,
                "record a failed cleanup revert mutation",
            )
            .await?;
        let commit = self
            .repository
            .commit_cleanup_revert_mutation(
                &applying.id,
                &prepared.mutation,
                outcome,
                &prepared.revert,
            )
            .await
            .map_err(|source| dependency("commit a cleanup revert mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(commit.affected_tracks)
    }

    pub(super) async fn recover_cleanup_mutations(&self) -> Result<(), LibraryCoordinatorError> {
        let entries = self
            .repository
            .unfinished_recovery_journals(RecoveryDomain::Cleanup)
            .await
            .map_err(|source| dependency("load unfinished cleanup mutations", source))?;
        let mut apply_mutations = Vec::new();
        let mut revert_mutations = Vec::new();
        let mut batch_reverts = Vec::new();
        for entry in entries {
            if entry.operation.as_str() == "revert_batch" {
                if entry.plan.get("cleanup_batch").is_some()
                    || entry.plan.get("cleanup_revert").is_some()
                {
                    return Err(LibraryCoordinatorError::RecoveryConflict);
                }
                batch_reverts.push(entry);
            } else if entry.plan.get("cleanup_revert").is_some() {
                revert_mutations.push(entry);
            } else if entry.plan.get("cleanup_batch").is_some() {
                apply_mutations.push(entry);
            } else {
                return Err(LibraryCoordinatorError::RecoveryConflict);
            }
        }
        for entry in apply_mutations {
            self.recover_cleanup_apply_mutation(entry).await?;
        }
        for entry in revert_mutations {
            self.recover_cleanup_revert_mutation(entry).await?;
        }
        for entry in batch_reverts {
            self.recover_cleanup_batch_revert(entry).await?;
        }
        Ok(())
    }

    async fn recover_cleanup_apply_mutation(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let mutation = LibraryFileMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let append = CleanupBatchAppend::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup mutation")
            .await?;
        let Some(outcome) = self
            .recover_cleanup_file_effect(&applying, mutation.clone())
            .await?
        else {
            return Ok(());
        };
        let commit = self
            .repository
            .commit_cleanup_mutation(&applying.id, &mutation, outcome, &append)
            .await
            .map_err(|source| dependency("commit a recovered cleanup mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(())
    }

    async fn recover_cleanup_revert_mutation(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let mutation = LibraryFileMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidMutation)?;
        let revert = CleanupRevertMutation::from_journal(&entry)
            .map_err(LibraryCoordinatorError::InvalidCleanupMutation)?;
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup revert mutation")
            .await?;
        let Some(outcome) = self
            .recover_cleanup_file_effect(&applying, mutation.clone())
            .await?
        else {
            return Ok(());
        };
        let commit = self
            .repository
            .commit_cleanup_revert_mutation(&applying.id, &mutation, outcome, &revert)
            .await
            .map_err(|source| dependency("commit a recovered cleanup revert mutation", source))?;
        self.status.send_replace(commit.status);
        Ok(())
    }

    async fn recover_cleanup_batch_revert(
        &self,
        entry: RecoveryJournalEntry,
    ) -> Result<(), LibraryCoordinatorError> {
        let plan = entry
            .plan
            .as_object()
            .filter(|plan| plan.len() == 1)
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        let batch_id = plan
            .get("batch_id")
            .and_then(Value::as_i64)
            .filter(|batch_id| *batch_id > 0)
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        let batch = self
            .repository
            .cleanup_batch(batch_id)
            .await
            .map_err(|source| dependency("load a recovering cleanup batch revert", source))?
            .ok_or(LibraryCoordinatorError::RecoveryConflict)?;
        if batch.reverted_at_unix_seconds.is_some() {
            return Err(LibraryCoordinatorError::RecoveryConflict);
        }
        let applying = self
            .resume_cleanup_mutation(entry, "resume a planned cleanup batch revert")
            .await?;
        let result = self
            .revert_cleanup_items(Some(batch_id), batch.items)
            .await?;
        self.repository
            .finish_cleanup_batch_revert(
                &applying.id,
                batch_id,
                result.reverted,
                result.skipped.len(),
            )
            .await
            .map_err(|source| dependency("finish a recovered cleanup batch revert", source))
    }

    async fn resume_cleanup_mutation(
        &self,
        entry: RecoveryJournalEntry,
        operation: &'static str,
    ) -> Result<RecoveryJournalEntry, LibraryCoordinatorError> {
        match entry.state {
            RecoveryState::Planned => transition_applied(
                self.repository
                    .transition_recovery_journal(
                        &entry.id,
                        RecoveryState::Planned,
                        RecoveryState::Applying,
                        json!({"recovered": true}),
                    )
                    .await
                    .map_err(|source| dependency(operation, source))?,
            ),
            RecoveryState::Applying => Ok(entry),
            RecoveryState::Committed
            | RecoveryState::RollingBack
            | RecoveryState::RolledBack
            | RecoveryState::Failed => Err(LibraryCoordinatorError::RecoveryConflict),
        }
    }

    async fn recover_cleanup_file_effect(
        &self,
        applying: &RecoveryJournalEntry,
        mutation: LibraryFileMutation,
    ) -> Result<Option<LibraryFileMutationOutcome>, LibraryCoordinatorError> {
        match self.effects.apply(&applying.id, mutation, true).await {
            Ok(outcome) => Ok(Some(outcome)),
            Err(failure) if !failure.requires_recovery() => {
                transition_applied(
                    self.repository
                        .transition_recovery_journal(
                            &applying.id,
                            RecoveryState::Applying,
                            RecoveryState::Failed,
                            json!({"error_code": failure.code(), "recovered": true}),
                        )
                        .await
                        .map_err(|source| {
                            dependency("record a failed recovered cleanup mutation", source)
                        })?,
                )?;
                Ok(None)
            }
            Err(failure) => Err(LibraryCoordinatorError::Mutation(failure)),
        }
    }

    // A recoverable failure must retain Applying so restart can finish its
    // disk/catalog transition. Only a failure with no recovery obligation closes it.
    async fn apply_cleanup_file_effect(
        &self,
        applying: &RecoveryJournalEntry,
        mutation: &LibraryFileMutation,
        failure_operation: &'static str,
    ) -> Result<LibraryFileMutationOutcome, LibraryCoordinatorError> {
        match self
            .effects
            .apply(&applying.id, mutation.clone(), false)
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(failure) => {
                if !failure.requires_recovery() {
                    transition_applied(
                        self.repository
                            .transition_recovery_journal(
                                &applying.id,
                                RecoveryState::Applying,
                                RecoveryState::Failed,
                                json!({"error_code": failure.code()}),
                            )
                            .await
                            .map_err(|source| dependency(failure_operation, source))?,
                    )?;
                }
                Err(LibraryCoordinatorError::Mutation(failure))
            }
        }
    }
}
