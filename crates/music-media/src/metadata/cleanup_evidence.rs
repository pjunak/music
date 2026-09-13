use super::{MetadataError, read_tagged_file};
use lofty::file::TaggedFileExt;
use lofty::tag::{ItemKey, Tag};
use music_application::cleanup_enrichment::evidence::{
    EvidenceField, LocalEvidence, LocalObservation, normalized_value,
};
use std::path::Path;

pub(super) fn protected_identifiers(
    tagged: &lofty::file::TaggedFile,
) -> std::collections::BTreeSet<(EvidenceField, String)> {
    tagged
        .tags()
        .iter()
        .flat_map(|tag| tag.items())
        .filter_map(|item| {
            let field = match item.key() {
                ItemKey::MusicBrainzRecordingId => EvidenceField::RecordingMbid,
                ItemKey::MusicBrainzReleaseId => EvidenceField::ReleaseMbid,
                ItemKey::MusicBrainzTrackId => EvidenceField::ReleaseTrackMbid,
                ItemKey::MusicBrainzReleaseGroupId => EvidenceField::ReleaseGroupMbid,
                ItemKey::Isrc => EvidenceField::Isrc,
                ItemKey::Barcode => EvidenceField::Barcode,
                ItemKey::CatalogNumber => EvidenceField::CatalogNumber,
                _ => return None,
            };
            // Preservation also covers malformed and excess IDs excluded from the
            // bounded retrieval evidence. A metadata edit must not destroy them.
            item.value().text().map(|value| (field, value.to_owned()))
        })
        .collect()
}

/// Lofty 0.25.1's generic ID3 conversion omits these mapped TXXX frames.
/// Reconstruct them before writing, preserving format-specific remainder frames.
pub(super) fn save_id3_with_identifiers(tag: &Tag, path: &Path) -> Result<(), MetadataError> {
    use lofty::TextEncoding;
    use lofty::id3::v2::{ExtendedTextFrame, Frame, Id3v2Tag};
    use lofty::tag::TagExt;
    let mut id3 = Id3v2Tag::from(tag.clone());
    for (key, description) in [
        (ItemKey::MusicBrainzReleaseId, "MusicBrainz Album Id"),
        (ItemKey::MusicBrainzTrackId, "MusicBrainz Release Track Id"),
        (
            ItemKey::MusicBrainzReleaseGroupId,
            "MusicBrainz Release Group Id",
        ),
    ] {
        let values = tag
            .get_items(key)
            .filter_map(|item| item.value().text())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            id3.insert(Frame::UserText(ExtendedTextFrame::new(
                TextEncoding::UTF8,
                description,
                values.join("\0"),
            )));
        }
    }
    id3.save_to_path(path, super::write_options())
        .map_err(|e| MetadataError::Write(e.to_string()))
}

/// Inspect every tag container without choosing or changing the authored primary tag.
pub fn read_cleanup_evidence(path: &Path) -> Result<LocalEvidence, MetadataError> {
    let tagged = read_tagged_file(path)?;
    Ok(evidence_from_tags(tagged.tags()))
}

fn evidence_from_tags(tags: &[Tag]) -> LocalEvidence {
    let mut result = LocalEvidence::default();
    for tag in tags.iter().take(8) {
        for item in tag.items() {
            let field = match item.key() {
                ItemKey::TrackTitle => EvidenceField::Title,
                ItemKey::TrackArtist => EvidenceField::Artist,
                ItemKey::AlbumArtist => EvidenceField::AlbumArtist,
                ItemKey::AlbumTitle => EvidenceField::Album,
                ItemKey::TrackNumber => EvidenceField::TrackNo,
                ItemKey::DiscNumber => EvidenceField::DiscNo,
                ItemKey::RecordingDate | ItemKey::ReleaseDate => EvidenceField::Date,
                ItemKey::OriginalReleaseDate => EvidenceField::OriginalDate,
                ItemKey::Composer => EvidenceField::Composer,
                ItemKey::Genre => EvidenceField::Genre,
                ItemKey::MusicBrainzRecordingId => EvidenceField::RecordingMbid,
                ItemKey::MusicBrainzReleaseId => EvidenceField::ReleaseMbid,
                ItemKey::MusicBrainzTrackId => EvidenceField::ReleaseTrackMbid,
                ItemKey::MusicBrainzReleaseGroupId => EvidenceField::ReleaseGroupMbid,
                ItemKey::Isrc => EvidenceField::Isrc,
                ItemKey::Barcode => EvidenceField::Barcode,
                ItemKey::CatalogNumber => EvidenceField::CatalogNumber,
                _ => continue,
            };
            if let Some(raw) = item.value().text()
                && normalized_value(field, raw).is_some()
            {
                result.observations.push(LocalObservation {
                    field,
                    value: raw.to_owned(),
                    source: format!("embedded {:?}", tag.tag_type()),
                });
            }
            if result.observations.len() == 128 {
                result
                    .notes
                    .push("Embedded evidence reached the 128-value limit.".into());
                return result;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::tag::TagType;
    #[test]
    fn format_fixtures_round_trip_recording_release_and_slot_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use lofty::tag::TagExt;
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/reference/v1/metadata.examples.json"
        ))?;
        let dir = tempfile::tempdir()?;
        let mut checked = 0;
        for case in corpus["cases"].as_array().ok_or("fixture missing cases")? {
            let extension = case["extension"]
                .as_str()
                .ok_or("fixture missing extension")?;
            if ![".mp3", ".flac", ".ogg", ".m4a"].contains(&extension) {
                continue;
            }
            let file = dir.path().join(format!("fixture{extension}"));
            std::fs::write(
                &file,
                STANDARD.decode(
                    case["source_base64"]
                        .as_str()
                        .ok_or("fixture missing media")?,
                )?,
            )?;
            let mut tagged = read_tagged_file(&file)?;
            let tag = tagged
                .primary_tag_mut()
                .ok_or("fixture primary tag absent")?;
            let recording = "00000000-0000-0000-0000-000000000001";
            let release = "00000000-0000-0000-0000-000000000002";
            let slot = "00000000-0000-0000-0000-000000000003";
            for (key, value) in [
                (ItemKey::MusicBrainzRecordingId, recording),
                (ItemKey::MusicBrainzReleaseId, release),
                (ItemKey::MusicBrainzTrackId, slot),
                (ItemKey::RecordingDate, "2026-09-10"),
            ] {
                // Recording IDs map to UFID specially; the generic ID3 key
                // lookup does not expose that mapping to insert_text.
                tag.insert_unchecked(lofty::tag::TagItem::new(
                    key,
                    lofty::tag::ItemValue::Text(value.into()),
                ));
            }
            if extension == ".mp3" {
                // Seed using raw frames, independently of the production repair.
                use lofty::TextEncoding;
                use lofty::id3::v2::{ExtendedTextFrame, Frame, Id3v2Tag};
                let mut native = Id3v2Tag::from(tag.clone());
                native.insert(Frame::UserText(ExtendedTextFrame::new(
                    TextEncoding::UTF8,
                    "MusicBrainz Album Id",
                    release,
                )));
                native.insert(Frame::UserText(ExtendedTextFrame::new(
                    TextEncoding::UTF8,
                    "MusicBrainz Release Track Id",
                    slot,
                )));
                native.save_to_path(&file, super::super::write_options())?;
            } else {
                use lofty::file::AudioFile;
                let mut reader = super::super::open_media_reader(&file)?;
                match extension {
                    ".flac" => {
                        let mut media = lofty::flac::FlacFile::read_from(
                            &mut reader,
                            super::super::parse_options(),
                        )?;
                        media.set_vorbis_comments(lofty::ogg::tag::VorbisComments::from(
                            tag.clone(),
                        ));
                        media.save_to_path(&file, super::super::write_options())?;
                    }
                    ".ogg" => {
                        let mut media = lofty::ogg::VorbisFile::read_from(
                            &mut reader,
                            super::super::parse_options(),
                        )?;
                        *media.vorbis_comments_mut() =
                            lofty::ogg::tag::VorbisComments::from(tag.clone());
                        media.save_to_path(&file, super::super::write_options())?;
                    }
                    _ => {
                        let mut media = lofty::mp4::Mp4File::read_from(
                            &mut reader,
                            super::super::parse_options(),
                        )?;
                        media.set_ilst(lofty::mp4::Ilst::from(tag.clone()));
                        media.save_to_path(&file, super::super::write_options())?;
                    }
                }
            }
            let observations = read_cleanup_evidence(&file)?;
            assert_eq!(
                observations.single(EvidenceField::RecordingMbid).as_deref(),
                Some(recording),
                "{extension}"
            );
            assert_eq!(
                observations.single(EvidenceField::ReleaseMbid).as_deref(),
                Some(release),
                "{extension}"
            );
            assert_eq!(
                observations
                    .single(EvidenceField::ReleaseTrackMbid)
                    .as_deref(),
                Some(slot),
                "{extension}"
            );
            assert_eq!(
                observations.single(EvidenceField::Date).as_deref(),
                Some("2026-09-10")
            );
            let mut patch = super::super::TagPatch::new();
            patch.insert_text(super::super::TagField::Title, "Reviewed title")?;
            let staged = dir.path().join(format!("staged{extension}"));
            let staged_update = super::super::stage_tag_update(&file, &staged, &patch)?;
            assert_eq!(
                protected_identifiers(&read_tagged_file(&file)?),
                protected_identifiers(&read_tagged_file(&staged)?)
            );
            drop(staged_update);
            checked += 1;
        }
        assert_eq!(checked, 4);
        Ok(())
    }
    #[test]
    fn secondary_tags_and_distinct_entity_ids_and_full_dates_survive() {
        let mut primary = Tag::new(TagType::Id3v2);
        primary.insert_text(ItemKey::TrackTitle, "Authored title".into());
        let mut secondary = Tag::new(TagType::Ape);
        let recording = "00000000-0000-0000-0000-000000000001";
        let slot = "00000000-0000-0000-0000-000000000002";
        secondary.insert_text(ItemKey::MusicBrainzRecordingId, recording.into());
        secondary.insert_text(ItemKey::MusicBrainzTrackId, slot.into());
        secondary.insert_text(ItemKey::RecordingDate, "2026-09-10".into());
        secondary.insert_text(ItemKey::TrackTitle, "Alternative title".into());
        let result = evidence_from_tags(&[primary, secondary]);
        assert_eq!(result.values(EvidenceField::Title).len(), 2);
        assert_eq!(
            result.single(EvidenceField::RecordingMbid).as_deref(),
            Some(recording)
        );
        assert_eq!(
            result.single(EvidenceField::ReleaseTrackMbid).as_deref(),
            Some(slot)
        );
        assert_eq!(
            result.single(EvidenceField::Date).as_deref(),
            Some("2026-09-10")
        );
    }
}
