use std::fmt::Debug;

use buffa::Message;
use serde::Serialize;
use serde::de::DeserializeOwned;
use veldmap_proto::veldmap::globe::v1::{OverlaySnapshot, OverlayStatus};
use veldmap_proto::veldmap::library::v1::{DownloadItem, DownloadSnapshot, DownloadStatus};
use veldmap_proto::veldmap::provider::v1::{
    ListStoragePathRequest, StorageEntry, StorageEntryResult, StorageListingMetadata,
};
use veldmap_proto::veldmap::tiler::v1::RasterVariable;
use veldmap_proto::veldmap::viewer::v1::{ViewSnapshot, ViewStatus};

#[test]
fn download_snapshot_round_trips_binary_and_json() {
    assert_round_trip(DownloadSnapshot {
        items: vec![DownloadItem {
            id: "download-1".into(),
            source_id: "source-1".into(),
            done_bytes: 512,
            total_bytes: 4096,
            status: DownloadStatus::Downloading.into(),
            modified_at_unix_ms: 1_761_000_123_456,
            product_id: "product-1".into(),
            sibling_count: 3,
            status_message: Some("Downloading part 2".into()),
            ..Default::default()
        }],
        ..Default::default()
    });
}

#[test]
fn view_snapshot_round_trips_binary_and_json() {
    assert_round_trip(ViewSnapshot {
        items: vec![ViewStatus {
            id: "view-1".into(),
            source_width: 10_980,
            source_height: 10_980,
            scale: 0.25,
            read_bytes: 2048,
            total_bytes: 8192,
            working: true,
            ready_tiles: 7,
            total_tiles: 16,
            step: 2,
            total_steps: 4,
            warning_message: Some("Using approximate geolocation".into()),
            active_variable: variable("B04", "Red").into(),
            candidate_variables: vec![variable("B04", "Red"), variable("B08", "NIR")],
            ..Default::default()
        }],
        ..Default::default()
    });
}

#[test]
fn overlay_snapshot_round_trips_binary_and_json() {
    assert_round_trip(OverlaySnapshot {
        items: vec![OverlayStatus {
            id: "overlay-1".into(),
            ready_tiles: 11,
            total_tiles: 20,
            working: true,
            progress_share: 0.55,
            step: 3,
            total_steps: 5,
            describing: true,
            pass_read_bytes: 1024,
            pass_total_bytes: 2048,
            detailed_ordinal: Some(2),
            detailed_warning: Some("Detailed raster is delayed".into()),
            general_warning: Some("Preview remains available".into()),
            detailed_variable: variable("B08", "NIR").into(),
            detailed_candidates: vec![variable("B08", "NIR"), variable("B04", "Red")],
            ..Default::default()
        }],
        ..Default::default()
    });
}

#[test]
fn empty_listing_metadata_round_trips_binary_and_json() {
    assert_round_trip(StorageEntryResult {
        outcome: StorageListingMetadata {
            truncated: false,
            ..Default::default()
        }
        .into(),
        ..Default::default()
    });
}

#[test]
fn complete_listing_round_trips_binary_and_json() {
    assert_round_trip(ListStoragePathRequest {
        path: "product-1".into(),
        recursive: true,
        limit: 100,
        ..Default::default()
    });
    assert_round_trip(StorageEntryResult {
        outcome: StorageEntry {
            key: "product-1/manifest.xml".into(),
            size_bytes: 4096,
            modified_at_unix_ms: 1_761_000_123_456,
            product_id: "product-1".into(),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    });
    assert_round_trip(StorageEntryResult {
        outcome: StorageListingMetadata {
            truncated: false,
            ..Default::default()
        }
        .into(),
        ..Default::default()
    });
}

#[test]
fn truncated_listing_metadata_round_trips_binary_and_json() {
    assert_round_trip(StorageEntryResult {
        outcome: StorageListingMetadata {
            truncated: true,
            ..Default::default()
        }
        .into(),
        ..Default::default()
    });
}

fn variable(path: &str, label: &str) -> RasterVariable {
    RasterVariable {
        path: path.into(),
        label: label.into(),
        units: "reflectance".into(),
        ..Default::default()
    }
}

fn assert_round_trip<M>(expected: M)
where
    M: Message + Serialize + DeserializeOwned + PartialEq + Debug,
{
    let encoded = expected.encode_to_vec();
    let binary = M::decode_from_slice(&encoded).expect("binary decode failed");
    assert_eq!(binary, expected);

    let json = serde_json::to_string(&expected).expect("JSON encode failed");
    let decoded: M = serde_json::from_str(&json).expect("JSON decode failed");
    assert_eq!(decoded, expected);
}
