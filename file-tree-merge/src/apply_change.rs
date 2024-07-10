use crate::change_tree::Change;
use crate::change_tree_merge::{Changes, HashKind, Items};
use crate::tree_creator::Item;
use crate::{crc_eq, file_hash, git_add, git_commit, TREE_DIR};
use anyhow::Result;
use colored::*;
use rayon::iter::ParallelIterator;
use rayon::prelude::IntoParallelRefIterator;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::{fs, io};

pub fn apply(
    changes: Changes,
    items_path1: Items,
    items_path2: Items,
    path1_mnt: &Path,
    path2_mnt: &Path,
    path1_repo: &Path,
    _path2_repo: &Path,
    dry_run: bool,
    checksum: bool,
    crc: bool,
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    println!(
        "{}",
        format!("Applying {} changes...", changes.len()).cyan()
    );
    let total = AtomicU64::new(0);
    let synced = AtomicU64::new(0);
    let applied_size_since_commit = AtomicU64::new(0);
    let items_path1 = Arc::new(Mutex::new(items_path1));
    let items_path2 = Arc::new(Mutex::new(items_path2));
    let mut to_process = vec![];
    let batch_size = 1000;
    let commit_after_size_bytes = 64 * 1024 * 1024;
    let git_lock = Arc::new(Mutex::new(()));
    for (change, path) in changes {
        to_process.push((change.clone(), path.to_string()));
        if to_process.len() % batch_size == 0 {
            // Process the collection in parallel
            let res: Vec<Result<()>> = to_process
                .par_iter() // Convert the vector into a parallel iterator
                .map(|(change, path)| {
                    process(
                        change,
                        path,
                        items_path1.clone(),
                        items_path2.clone(),
                        path1_mnt,
                        path2_mnt,
                        path1_repo,
                        _path2_repo,
                        dry_run,
                        checksum,
                        crc,
                        &total,
                        git_lock.clone(),
                        batch_size,
                        &synced,
                        &applied_size_since_commit,
                        commit_after_size_bytes,
                    )
                })
                .collect();
            for e in res {
                if let Err(err) = e {
                    println!(
                        "{}",
                        format!(
                            "Synced {}/{}",
                            synced.load(Ordering::SeqCst),
                            total.load(Ordering::SeqCst)
                        )
                        .green()
                        .bold()
                    );
                    Err(err)?
                }
            }
            to_process.clear();
        }
    }
    // Process the collection in parallel
    let res: Vec<Result<()>> = to_process
        .par_iter() // Convert the vector into a parallel iterator
        .map(|(change, path)| {
            process(
                change,
                path,
                items_path1.clone(),
                items_path2.clone(),
                path1_mnt,
                path2_mnt,
                path1_repo,
                _path2_repo,
                dry_run,
                checksum,
                crc,
                &total,
                git_lock.clone(),
                batch_size,
                &synced,
                &applied_size_since_commit,
                commit_after_size_bytes,
            )
        })
        .collect();
    for e in res {
        if let Err(err) = e {
            if !dry_run {
                println!(
                    "{}",
                    format!(
                        "Synced {}/{}",
                        synced.load(Ordering::SeqCst),
                        total.load(Ordering::SeqCst)
                    )
                    .green()
                    .bold()
                );
            }
            Err(err)?
        }
    }
    if !dry_run {
        println!(
            "{}",
            format!(
                "Synced {}/{}",
                synced.load(Ordering::SeqCst),
                total.load(Ordering::SeqCst)
            )
            .green()
            .bold()
        );
    }
    git_add(&path1_repo.join(TREE_DIR), ".")?;
    git_commit(path1_repo)?;

    Ok(())
}

fn items_content_eq(
    path1_mnt: &&Path,
    a: &Item,
    path2_mnt: &&Path,
    b: &Item,
    checksum: bool,
) -> io::Result<bool> {
    if a.size == b.size && a.mtime == b.mtime {
        if checksum {
            let hash1 = file_hash(&path1_mnt.join(&a.path), HashKind::Md5)?;
            let hash2 = file_hash(&path2_mnt.join(&b.path), HashKind::Md5)?;
            Ok(hash1.eq(&hash2))
        } else {
            Ok(true)
        }
    } else {
        Ok(false)
    }
}

fn process(
    change: &Change,
    path: &String,
    items_path1: Arc<Mutex<Items>>,
    items_path2: Arc<Mutex<Items>>,
    path1_mnt: &Path,
    path2_mnt: &Path,
    path1_repo: &Path,
    _path2_repo: &Path,
    dry_run: bool,
    checksum: bool,
    crc: bool,
    ctr: &AtomicU64,
    git_lock: Arc<Mutex<()>>,
    batch_size: usize,
    synced: &AtomicU64,
    applied_size_since_commit: &AtomicU64,
    commit_after_size_bytes: i32,
) -> Result<()> {
    let path2 = path2_mnt.join(path);
    ctr.fetch_add(1, Ordering::SeqCst);
    match change.clone() {
        Change::Add | Change::Modify => {
            if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                if matches!(change, Change::Add) {
                    println!("{} '{}'", change.to_string().green(), path.green());
                } else {
                    println!("{} '{}'", change.to_string().blue(), path.blue());
                }
            }
            // check if it's the same as in dst
            let mut add = true;
            let guard = items_path1.lock().unwrap();
            let path1_item = guard.get(path).unwrap();
            if let Some(dst_item) = items_path2.lock().unwrap().get(path) {
                if items_content_eq(&path1_mnt, &path1_item, &path2_mnt, &dst_item, checksum)? {
                    add = false;
                }
            }
            if add {
                if dry_run {
                    return Ok(());
                }
                fs::create_dir_all(path2.parent().unwrap())?;
                fs::copy(path1_mnt.join(&path), path2.clone())?;
                File::set_times(&File::open(path2.clone())?, path1_item.times)?;
                File::open(path2.clone())?.sync_all()?;
                File::open(path2.parent().unwrap())?.sync_all()?;
                if crc && !crc_eq(&path1_mnt.join(&path), &path2.clone())? {
                    // todo: collect in errors
                    println!(
                        "{}",
                        "   CRC check failed after transfer, aborting".red().bold()
                    );
                    anyhow::bail!("CRC check failed for `{path}` after transfer");
                }
                synced.fetch_add(1, Ordering::SeqCst);
                applied_size_since_commit.fetch_add(path1_item.size, Ordering::SeqCst);
            } else if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                println!(
                    "{}",
                    "   skip, already present in path2 with the same content".yellow()
                );
            }
        }
        Change::Delete => {
            if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                println!("{} '{}'", change.to_string().red(), path.red().bold());
            }
            if path2.exists() {
                if dry_run {
                    return Ok(());
                }
                fs::remove_file(path2.clone())?;
                File::open(path2.parent().unwrap())?.sync_all()?;
            } else if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                println!("{}", "  skip, not present in path2".yellow());
            }
            synced.fetch_add(1, Ordering::SeqCst);
        }
        Change::Rename(old_path) => {
            if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                println!("{} '{}'", change.to_string().magenta(), path.magenta());
            }
            let guard = items_path1.lock().unwrap();
            let path1_item = guard.get(path).unwrap();
            // todo: compare if old file hash in src is same as old file hash in dst
            if path2_mnt.join(&old_path).exists() {
                if dry_run {
                    return Ok(());
                }
                fs::create_dir_all(path2.parent().unwrap())?;
                fs::rename(path2_mnt.join(&old_path), path2.clone())?;
                File::set_times(&File::open(path2.clone())?, path1_item.times)?;
                File::open(path2.clone())?.sync_all()?;
                File::open(path2.parent().unwrap())?.sync_all()?;
            } else {
                println!("{}", format!("  cannot R '{old_path}' -> '{path}', old file not present in path2. Will copy instead from path1 to the new destination").yellow());
                if dry_run {
                    return Ok(());
                }
                fs::create_dir_all(path2_mnt.join(path).parent().unwrap())?;
                fs::copy(path1_mnt.join(path), path2.clone())?;
                File::set_times(&File::open(path2.clone())?, path1_item.times)?;
                File::open(path2.clone())?.sync_all()?;
                File::open(path2.parent().unwrap())?.sync_all()?;
                if crc && !crc_eq(&path1_mnt.join(path), &path2.clone())? {
                    // todo: collect in errors
                    println!(
                        "{}",
                        "   CRC check failed after transfer, aborting".red().bold()
                    );
                    anyhow::bail!("CRC check failed for `{path}` after transfer");
                }
                applied_size_since_commit.fetch_add(path1_item.size, Ordering::SeqCst);
            }
            synced.fetch_add(1, Ordering::SeqCst);
        }
        Change::Copy(old_path) => {
            if ctr.load(Ordering::SeqCst) % batch_size as u64 == 0 {
                println!("{} '{}'", change.to_string().blue(), path.blue());
            }
            let guard = items_path1.lock().unwrap();
            let path1_item = guard.get(path).unwrap();
            // todo: compare if old file hash in src is same as old file hash in dst
            if path2_mnt.join(&old_path).exists() {
                if dry_run {
                    return Ok(());
                }
                fs::create_dir_all(path2.clone().parent().unwrap())?;
                fs::copy(path2_mnt.join(&old_path), path2.clone())?;
                File::set_times(&File::open(path2.clone())?, path1_item.times)?;
                File::open(path2.clone())?.sync_all()?;
                File::open(path2.parent().unwrap())?.sync_all()?;
            } else {
                println!("{}", format!("  cannot C '{old_path}' -> '{path}', old file not present in path2. Will copy instead from path1 to the new destination").yellow());
                if dry_run {
                    return Ok(());
                }
                fs::create_dir_all(path2.parent().unwrap())?;
                fs::copy(path1_mnt.join(path), path2.clone())?;
            }
            if crc && !crc_eq(&path1_mnt.join(path), &path2.clone())? {
                // todo: collect in errors
                println!(
                    "{}",
                    "   CRC check failed after transfer, aborting".red().bold()
                );
                anyhow::bail!("CRC check failed for `{path}` after transfer");
            }
            synced.fetch_add(1, Ordering::SeqCst);
            applied_size_since_commit.fetch_add(path1_item.size, Ordering::SeqCst);
        }
    }
    let _guard = git_lock.lock().unwrap();
    git_add(&path1_repo.join(TREE_DIR), path)?;
    if applied_size_since_commit.load(Ordering::SeqCst) > commit_after_size_bytes as u64 {
        println!("{}", "Checkpointing applied changes...".cyan());
        git_commit(path1_repo)?;
        applied_size_since_commit.store(0, Ordering::SeqCst);
    }
    Ok(())
}
