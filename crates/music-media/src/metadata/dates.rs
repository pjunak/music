use super::{AudioMetadata, MetadataError, TagField, TagPatch, TagValue};

/// The legacy year field aliases the date tag. Unrelated/unchanged year edits must
/// not discard known precision; changing a precise date requires an explicit date.
pub(super) fn resolve_patch(
    patch: &TagPatch,
    current: &AudioMetadata,
) -> Result<TagPatch, MetadataError> {
    let mut resolved = patch.clone();
    if let Some(year) = patch.changes.get(&TagField::Year) {
        let requested = match year {
            Some(TagValue::Number(value)) => Some(*value),
            _ => None,
        };
        if let Some(date) = patch.changes.get(&TagField::ReleaseDate) {
            let date_year = match date {
                Some(TagValue::Text(value)) => music_domain::metadata_date_year(value),
                _ => None,
            };
            if requested != date_year {
                return Err(MetadataError::ConflictingDate);
            }
        } else {
            let value = if requested.is_some()
                && requested == current.year
                && !current.release_date.is_empty()
            {
                Some(TagValue::Text(current.release_date.clone()))
            } else {
                if current.release_date.len() > 4 {
                    return Err(MetadataError::DatePrecisionLoss);
                }
                requested.map(|year| TagValue::Text(format!("{year:04}")))
            };
            resolved.changes.insert(TagField::ReleaseDate, value);
        }
        resolved.changes.remove(&TagField::Year);
    }
    Ok(resolved)
}
