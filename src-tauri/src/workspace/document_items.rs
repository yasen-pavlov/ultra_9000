//! Document rows use the same selection and presentation authority, without a PTY or tool.
use super::*;

pub fn open_document(loaded: Loaded, project: String, select_item: bool) -> Result<String, String> {
    with(|w| {
        if w.quitting {
            return Err("Ultra 9000 is quitting.".into());
        }
        if let Some(index) = w.records.iter().position(|r| {
            r.state
                .document
                .as_ref()
                .is_some_and(|d| d.path == loaded.state.path)
        }) {
            let id = w.records[index].state.id.clone();
            if loaded.state.follow {
                w.records[index].state.document.as_mut().unwrap().follow = true;
                w.changed = true;
            }
            if select_item {
                w.select(&id)?;
            }
            return Ok(id);
        }
        let at = now();
        let id = format!("item-{}", w.next_item);
        w.next_item += 1;
        let path = Path::new(&loaded.state.path);
        let state = ItemState {
            id: id.clone(),
            tool: "document".into(),
            kind: ItemKind::Document,
            project_id: project.clone(),
            cwd: path.parent().unwrap().to_string_lossy().into_owned(),
            title: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            process: None,
            document: Some(loaded.state.clone()),
            exit_code: None,
            failure: None,
            activity: Activity::Unknown,
            attention: None,
            unread: false,
            conversation: None,
            model: None,
            started_at: at,
            state_since: at,
            pid: None,
            foreground_job: None,
            background_jobs: Vec::new(),
        };
        w.records.push(Record {
            key: None,
            state,
            spec: None,
            omp: None,
            raw_title: String::new(),
            conversation_title: None,
            closing: false,
            extra: None,
            job_candidate: None,
        });
        if !w.project_order.contains(&project) {
            w.project_order.push(project);
        }
        w.host.watch_document(id.clone(), loaded);
        w.changed = true;
        if select_item {
            w.select(&id)?;
        }
        Ok(id)
    })
}
pub fn apply_document(id: &str, mut state: DocumentState) {
    let _ = with(|w| {
        let Ok(index) = w.index(id) else {
            return Ok(());
        };
        if let Some(current) = &w.records[index].state.document {
            if current.path == state.path && state.revision >= current.revision {
                state.follow = current.follow;
                w.changed |= current != &state;
                w.records[index].state.document = Some(state);
            }
        }
        Ok(())
    });
}
pub fn document_path(id: &str) -> Result<String, String> {
    with(|w| {
        w.records[w.index(id)?]
            .state
            .document
            .as_ref()
            .map(|d| d.path.clone())
            .ok_or_else(|| "Item is not a document.".into())
    })
}
pub fn project_document_epoch(project: &str) -> Result<u64, String> {
    with(|w| {
        let listed = w
            .settings
            .pinned
            .iter()
            .any(|path| path == Path::new(project))
            || w.records.iter().any(|r| r.state.project_id == project);
        if !listed {
            return Err("Project does not exist.".into());
        }
        Ok(w.epoch)
    })
}
pub fn new_neovim_for_file(id: &str) -> Result<String, String> {
    let (path, cwd, project, existing) = with(|w| {
        let item = &w.records[w.index(id)?].state;
        let path = item
            .document
            .as_ref()
            .ok_or("Item is not a document.")?
            .path
            .clone();
        let existing = w
            .records
            .iter()
            .find(|r| {
                r.state.tool == "neovim"
                    && r.state.project_id == item.project_id
                    && matches!(r.state.process, Some(Process::Starting | Process::Running))
                    && r.extra
                        .as_ref()
                        .is_some_and(|args| args.as_slice() == ["--", path.as_str()])
            })
            .map(|r| r.state.id.clone());
        Ok((path, item.cwd.clone(), item.project_id.clone(), existing))
    })?;
    if let Some(existing) = existing {
        select(&existing)?;
        return Ok(existing);
    }
    begin_launch(
        "neovim".into(),
        cwd,
        project,
        None,
        None,
        None,
        Some(vec!["--".into(), path]),
    )
}
pub fn reveal_document(id: &str) -> Result<(), String> {
    let (host, path) = with(|w| {
        let item = &w.records[w.index(id)?].state;
        let document = item.document.as_ref().ok_or("Item is not a document.")?;
        let path = if document.status == documents::DocumentStatus::Missing {
            Path::new(&document.path)
                .parent()
                .ok_or("Document has no directory.")?
                .to_string_lossy()
                .into_owned()
        } else {
            document.path.clone()
        };
        Ok((w.host.clone(), path))
    })?;
    host.reveal_file(path);
    Ok(())
}

pub fn has_quit_items() -> Result<bool, String> {
    with(|w| {
        Ok(w.records.iter().any(|r| {
            r.state.document.is_some()
                || matches!(r.state.process, Some(Process::Starting | Process::Running))
        }))
    })
}
