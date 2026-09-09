//! Hash every ordinary file without writing its payload back to disk. Native tar
//! still restores the real archived ACLs, xattrs, links and flags on small probes.
//! Kernel-compressed files store their content in xattrs; these remain intact and
//! are read through macOS after extraction, with the same strict metadata budget.
use super::*;
use crate::restore::{inspect_archive, open_archive, relative_path, require_root};
use std::io;

const MAX_METADATA: u64 = 512 * 1024 * 1024;
const RESERVE: u64 = 2 * 1024 * 1024 * 1024;

pub(super) fn space_preflight() -> Result<(), String> {
    require_free_space(&std::env::temp_dir(), RESERVE)
}

struct LimitedMetadata<W> { inner: W, written: u64, checked: u64, directory: PathBuf }
impl<W: Write> Write for LimitedMetadata<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self.written.saturating_add(bytes.len() as u64);
        if next > MAX_METADATA {
            return Err(io::Error::other("Readback metadata exceeds the 512 MiB temporary limit; no full file copy was created"));
        }
        if next.saturating_sub(self.checked) >= 1024 * 1024 {
            require_free_space(&self.directory, RESERVE.saturating_add(next.saturating_mul(3)))
                .map_err(io::Error::other)?;
            self.checked = next;
        }
        let n = self.inner.write(bytes)?;
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> { self.inner.flush() }
}

pub(super) fn verify_contents_and_metadata(archive: &Path, root: &str, stage: &Path) -> Result<Vec<ManifestEntry>, String> {
    space_preflight()?;
    // Validate the original archive, including duplicate names, traversal, links,
    // the metadata marker and compression checksum before any native extraction.
    let (index, flags) = inspect_archive(archive)?;
    require_root(&index, std::ffi::OsStr::new(root))?;
    let pax = flags.as_ref().is_some_and(|(_,f)|f.pax_metadata);
    let marker = flags.as_ref().map(|(p,_)|p.as_path());
    let metadata = stage.join("metadata.tar.gz");
    let file = fs::File::create(&metadata).map_err(|e|fail(&metadata,e))?;
    let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let limited = LimitedMetadata {inner:gzip, written:0, checked:0, directory:stage.into()};
    let mut output = tar::Builder::new(limited);
    let mut contents: BTreeMap<PathBuf,(u64,String)> = BTreeMap::new();
    let mut hardlinks = Vec::new();
    let mut input = open_archive(archive)?;
    {
        let _phase=crate::work_progress::Phase::enter("Archiv-Dateiinhalte im Datenstrom prüfen");
        let mut tar = tar::Archive::new(&mut input);
        let mut buffer = vec![0;1024*1024];
        for entry in tar.entries().map_err(|e|fail(archive,e))? {
            cancelled()?;
            let mut entry=entry.map_err(|e|fail(archive,e))?;
            let path=relative_path(&entry.path().map_err(|e|fail(archive,e))?)?;
            let mut header=entry.header().clone();
            let kind=header.entry_type();
            let link=entry.link_name().map_err(|e|fail(archive,e))?.map(|p|p.into_owned());
            // Old copyfile archives carry their metadata in AppleDouble payloads.
            // New PAX archives treat every ._ filename as ordinary user content.
            let keep_payload=marker==Some(path.as_path()) || (!pax && path.file_name().is_some_and(|n|n.as_bytes().starts_with(b"._")));
            let mut extensions=Vec::new();
            if let Some(pax)=entry.pax_extensions().map_err(|e|fail(archive,e))? {
                for item in pax {
                    let item=item.map_err(|e|fail(archive,e))?;
                    let key=item.key().map_err(|e|fail(archive,e))?;
                    // Data length is the only altered property of ordinary probes.
                    if key=="size" && kind.is_file() && !keep_payload { continue; }
                    extensions.push((key.to_owned(),item.value_bytes().to_vec()));
                }
            }
            if !extensions.is_empty() {
                output.append_pax_extensions(extensions.iter().map(|(k,v)|(k.as_str(),v.as_slice())))
                    .map_err(|e|fail(archive,e))?;
            }
            if kind.is_file() && !keep_payload {
                let mut digest=Sha256::new();
                let mut size=0u64;
                loop {
                    cancelled()?;
                    let n=entry.read(&mut buffer).map_err(|e|fail(archive,e))?;
                    if n==0 {break;}
                    digest.update(&buffer[..n]);size=size.saturating_add(n as u64);
                }
                contents.insert(path.clone(),(size,format!("{:x}",digest.finalize())));
                // A compressed flag needs a non-empty compressible probe for
                // macOS to actually recreate UF_COMPRESSED. Hashes above still
                // cover all original bytes, never these synthetic probe bytes.
                let compressed=extensions.iter().any(|(k,v)|k=="SCHILY.fflags" && v.split(|b|*b==b',').any(|f|f==b"compressed"));
                let probe=if compressed {vec![0u8;4096]} else {Vec::new()};
                header.set_size(probe.len() as u64);header.set_cksum();
                output.append_data(&mut header,&path,probe.as_slice()).map_err(|e|fail(archive,e))?;
            } else if let Some(link)=link {
                if kind.is_hard_link() {hardlinks.push((path.clone(),relative_path(&link)?));}
                output.append_link(&mut header,&path,&link).map_err(|e|fail(archive,e))?;
            } else {
                output.append_data(&mut header,&path,&mut entry).map_err(|e|fail(archive,e))?;
            }
        }
    }
    input.finish()?;
    let limited=output.into_inner().map_err(|e|fail(archive,e))?;
    let metadata_bytes=limited.written;
    limited.inner.finish().map_err(|e|fail(archive,e))?.sync_all().map_err(|e|fail(archive,e))?;
    for (path,link) in hardlinks {
        if let Some(content)=contents.get(&link).cloned() {contents.insert(path,content);}
    }
    // Allow for both xattr encodings, filesystem blocks and small directory nodes.
    // No regular payload is materialized; the system disk retains a 2 GiB reserve.
    let extraction_budget=metadata_bytes.saturating_mul(3).saturating_add((index.len() as u64).saturating_mul(16384));
    require_free_space(stage,RESERVE.saturating_add(extraction_budget))?;
    let probes=stage.join("probes");fs::create_dir(&probes).map_err(|e|fail(stage,e))?;
    unpack_private_with_root(&metadata,&probes,Some(std::ffi::OsStr::new(root)))?;
    let mut actual=snapshot_with_phase(&probes.join(root),"Rückgelesene Dateiattribute prüfen")?;
    for entry in &mut actual {
        if entry.kind!="file" {continue;}
        let path=Path::new(root).join(&entry.p);
        if entry.flags & libc::UF_COMPRESSED != 0 {
            // macOS has reconstructed these bytes from the original decmpfs and
            // resource-fork xattrs. Keep its actual logical size and content hash.
            if let Some((size,hash))=contents.get(&path).filter(|(size,_)|*size!=0) {
                if entry.s!=4096 {return Err(fail(archive,"Unexpected compressed metadata probe size"));}
                entry.s=*size;entry.hash=hash.clone();
            }
        } else if let Some((size,hash))=contents.get(&path) {
            if entry.s!=0 {return Err(fail(archive,"Unexpected payload in metadata probe"));}
            entry.s=*size;entry.hash=hash.clone();
        }
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_payload_is_fully_verified_without_a_full_temporary_copy() {
        let owned=ReadbackDir(PrivateDir::temp().unwrap());
        let source=owned.0 .0.join("virtual-disk.hds");
        let mut file=fs::File::create(&source).unwrap();
        file.set_len(64*1024*1024).unwrap();
        file.write_all(b"nonzero beginning of sparse virtual disk").unwrap();
        drop(file);
        xattr::set(&source,"com.example.backup",b"must survive").unwrap();
        let expected=compute_snapshot(&source).unwrap();
        let archive=owned.0 .0.join("disk.tar.gz");
        create_verified_archive_from_snapshot(&source,&archive,true,&expected).unwrap();
        let stage=owned.0 .0.join("readback");fs::create_dir(&stage).unwrap();
        let actual=verify_contents_and_metadata(&archive,"virtual-disk.hds",&stage).unwrap();
        assert!(readback_differences(&actual[0],&expected[0]).is_empty());
        assert_eq!(fs::metadata(stage.join("probes/virtual-disk.hds")).unwrap().len(),0);
        let allocated: u64=WalkDir::new(&stage).into_iter().map(|e|fs::symlink_metadata(e.unwrap().path()).unwrap().blocks()*512).sum();
        assert!(allocated<2*1024*1024,"temporary allocation: {allocated}");
        assert_eq!(fs::metadata(&source).unwrap().len(),64*1024*1024);
    }
    #[test]
    fn same_size_wrong_payload_is_not_hidden_by_metadata_probes() {
        let owned=ReadbackDir(PrivateDir::temp().unwrap());
        let source=owned.0 .0.join("file");fs::write(&source,b"original").unwrap();
        let expected=compute_snapshot(&source).unwrap();
        fs::write(&source,b"tampered").unwrap();
        let archive=owned.0 .0.join("changed.tar.gz");create_verified_archive(&source,&archive,true).unwrap();
        assert!(verify_archive_source(&archive,"file",&expected).unwrap_err().contains("SHA-256"));
    }
    #[test]
    fn metadata_budget_rejects_before_writing_past_limit() {
        let mut writer=LimitedMetadata {inner:Vec::new(),written:MAX_METADATA-3,checked:MAX_METADATA-3,directory:std::env::temp_dir()};
        assert!(writer.write_all(b"four").unwrap_err().to_string().contains("512 MiB"));
        assert!(writer.inner.is_empty());
    }
}
