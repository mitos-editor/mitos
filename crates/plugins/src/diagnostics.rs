//! Small configured-package metadata survives failed transactional replacements.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
};

use plugin_api::{
    diagnostics::{
        DiagnosticEntry, DiagnosticLevel, PluginDiagnostics, PluginEngine, PluginStatus,
        RequestTimings, MAX_DIAGNOSTIC_ENTRIES,
    },
    CapabilitySet,
};

use crate::PluginConfig;

#[derive(Default)]
pub(crate) struct Catalog {
    records: Mutex<BTreeMap<String, Record>>,
}

struct Record {
    generation: u64,
    status: PluginStatus,
    engine: Option<PluginEngine>,
    declared: CapabilitySet,
    granted: CapabilitySet,
    sequence: u64,
    entries: VecDeque<DiagnosticEntry>,
}

impl Catalog {
    pub fn begin(&self, configs: &BTreeMap<String, PluginConfig>, generation: u64) {
        let mut records = self.records.lock().unwrap();
        let mut previous = std::mem::take(&mut *records);
        for (name, config) in configs {
            let prior = previous.remove(name);
            records.insert(
                name.clone(),
                Record {
                    generation,
                    status: if config.enabled {
                        PluginStatus::Preparing
                    } else {
                        PluginStatus::Disabled
                    },
                    engine: None,
                    declared: CapabilitySet::new(),
                    granted: config.permissions.capabilities.clone(),
                    sequence: prior.as_ref().map_or(0, |record| record.sequence),
                    entries: prior.map_or_else(VecDeque::new, |record| record.entries),
                },
            );
        }
    }

    pub fn loaded(&self, name: &str, declared: CapabilitySet, executable: bool) {
        if let Some(record) = self.records.lock().unwrap().get_mut(name) {
            record.declared = declared;
            record.engine = Some(if executable {
                PluginEngine::Component
            } else {
                PluginEngine::Declarative
            });
        }
    }

    pub fn ready(&self) {
        for record in self.records.lock().unwrap().values_mut() {
            if !matches!(record.status, PluginStatus::Disabled) {
                record.status = PluginStatus::Ready;
            }
        }
    }

    pub fn failed(&self, message: &str) {
        for record in self.records.lock().unwrap().values_mut() {
            if matches!(record.status, PluginStatus::Disabled) {
                continue;
            }
            record.status = PluginStatus::Failed;
            record.sequence = record.sequence.saturating_add(1);
            if record.entries.len() == MAX_DIAGNOSTIC_ENTRIES {
                record.entries.pop_front();
            }
            record.entries.push_back(DiagnosticEntry::bounded(
                record.sequence,
                DiagnosticLevel::Error,
                &format!(
                    "Generation {} preparation failed: {message}",
                    record.generation
                ),
            ));
        }
    }

    pub fn snapshot(&self) -> BTreeMap<String, PluginDiagnostics> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|(name, record)| {
                (
                    name.clone(),
                    PluginDiagnostics {
                        plugin: name.clone(),
                        generation: record.generation,
                        status: record.status,
                        engine: record.engine,
                        declared: record.declared.clone(),
                        effective: record
                            .declared
                            .intersection(&record.granted)
                            .copied()
                            .collect(),
                        queued: 0,
                        timings: RequestTimings::default(),
                        entries: record.entries.iter().cloned().collect(),
                    },
                )
            })
            .collect()
    }
}
