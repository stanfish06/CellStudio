use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderName, StatusCode};
use axum::response::Response;
use bytes::{BufMut, BytesMut};
use cellstudio_core::LayerId;
use cellstudio_core::axes::Dtype;
use cellstudio_core::reader::OrthoAxis;
use serde::Deserialize;

use crate::error::{ApiError, ApiResult};
use crate::routes::project::{session_json, session_response};
use crate::state::AppState;
use crate::wire::{
    DTYPE_HEADER, HistogramWire, LEVEL_HEADER, PixelValue, SHAPE_HEADER, VOLUME_SOURCE_HEADER,
};

#[derive(Debug, Deserialize)]
pub struct SliceQuery {
    #[serde(default)]
    layer: Option<LayerId>,
    axis: String,
    t: u64,
    /// Visible channels, comma separated.
    cs: String,
    pos: u64,
    #[serde(default)]
    level: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct VolumeQuery {
    #[serde(default)]
    layer: Option<LayerId>,
    t: u64,
    c: u64,
    #[serde(default)]
    level: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PixelQuery {
    #[serde(default)]
    layer: Option<LayerId>,
    t: u64,
    c: u64,
    z: u64,
    y: u64,
    x: u64,
}

#[derive(Debug, Deserialize)]
pub struct HistogramQuery {
    #[serde(default)]
    layer: Option<LayerId>,
    t: u64,
    c: u64,
}

/// One XZ or YZ plane per requested channel.
pub async fn slice(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SliceQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let axis = match query.axis.to_ascii_lowercase().as_str() {
        // the image's XY tiles still come from /store, where viv's multiscale layer reads
        // them; this path serves label planes and anything else assembled (design M15)
        "xy" => OrthoAxis::XY,
        "xz" => OrthoAxis::XZ,
        "yz" => OrthoAxis::YZ,
        other => {
            return Err(ApiError::BadRequest(format!(
                "axis must be xy, xz or yz, got `{other}`"
            )));
        }
    };
    let channels = parse_channels(&query.cs)?;
    let layer = query.layer.unwrap_or(LayerId::Image);
    let level = query.level.unwrap_or(0);

    let reader = active.image.clone();
    let labels = active.coordinator.clone();
    let count = channels.len();
    let (bytes, shape, dtype) = state
        .read(move || {
            let _labels = (layer == LayerId::Labels).then(|| labels.read_labels());
            let mut packed: Option<([u32; 2], Dtype, BytesMut)> = None;
            for c in channels {
                let plane = reader.read_ortho_plane(layer, level, axis, query.t, c, query.pos)?;
                match packed.as_mut() {
                    None => {
                        let mut buffer = BytesMut::with_capacity(plane.bytes.len() * count);
                        buffer.put_slice(&plane.bytes);
                        packed = Some((plane.shape, plane.dtype, buffer));
                    }
                    Some((shape, dtype, buffer)) => {
                        if *shape != plane.shape || *dtype != plane.dtype {
                            return Err(ApiError::Internal(
                                "channels of one plane disagree on shape or dtype".to_owned(),
                            ));
                        }
                        buffer.put_slice(&plane.bytes);
                    }
                }
            }
            let (shape, dtype, buffer) =
                packed.ok_or_else(|| ApiError::BadRequest("cs lists no channels".to_owned()))?;
            Ok((buffer.freeze(), shape, dtype))
        })
        .await?;

    let shape = format!("{},{},{}", count, shape[0], shape[1]);
    Ok(session_response(
        &active.session_id,
        binary(bytes, &shape, dtype, level),
    ))
}

pub async fn volume(
    State(state): State<Arc<AppState>>,
    Query(query): Query<VolumeQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let layer = query.layer.unwrap_or(LayerId::Image);
    let level = query.level.unwrap_or(0);
    let reader = active.image.clone();
    let labels = active.coordinator.clone();
    let volume = state
        .read(move || {
            let _labels = (layer == LayerId::Labels).then(|| labels.read_labels());
            Ok(reader.read_volume(layer, level, query.t, query.c)?)
        })
        .await?;

    let shape = format!(
        "{},{},{}",
        volume.shape[0], volume.shape[1], volume.shape[2]
    );
    let mut response = binary(volume.bytes, &shape, volume.dtype, volume.level);
    let source = if volume.from_proxy {
        "proxy"
    } else {
        "pyramid"
    };
    response.headers_mut().insert(
        HeaderName::from_static(VOLUME_SOURCE_HEADER),
        axum::http::HeaderValue::from_static(source),
    );
    Ok(session_response(&active.session_id, response))
}

/// Cursor readout.
pub async fn pixel(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PixelQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let layer = query.layer.unwrap_or(LayerId::Image);
    let reader = active.image.clone();
    let labels = active.coordinator.clone();
    let value = state
        .read(move || {
            // a label read never sees a plane mixed from pre- and post-edit bricks; image
            // reads stay lock-free, since nothing writes them (design M9)
            let _labels = (layer == LayerId::Labels).then(|| labels.read_labels());
            Ok(reader.read_pixel(layer, query.t, query.c, [query.z, query.y, query.x])?)
        })
        .await?;
    Ok(session_json(&active.session_id, PixelValue { value }))
}

pub async fn histogram(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HistogramQuery>,
) -> ApiResult<Response> {
    let active = state.require()?;
    let layer = query.layer.unwrap_or(LayerId::Image);
    let reader = active.image.clone();
    let (histogram, level_voxels) = state
        .read(move || {
            let histogram = reader.histogram(layer, query.t, query.c)?;
            let dataset = reader
                .bricks()
                .layer(layer)
                .ok_or_else(|| ApiError::NotFound(format!("layer {layer:?} is not open")))?;
            let voxels = dataset.level(histogram.level)?.dims.zyx_voxels();
            Ok((histogram, voxels))
        })
        .await?;

    let level = histogram.level;
    let mut response = session_json(
        &active.session_id,
        HistogramWire::new(&histogram, level_voxels),
    );
    response.headers_mut().insert(
        HeaderName::from_static(LEVEL_HEADER),
        header_number(level as u64),
    );
    Ok(response)
}

fn parse_channels(cs: &str) -> ApiResult<Vec<u64>> {
    let channels: Result<Vec<u64>, _> = cs
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::parse::<u64>)
        .collect();
    let channels = channels.map_err(|_| {
        ApiError::BadRequest(format!(
            "cs must be comma-separated channel indices, got `{cs}`"
        ))
    })?;
    if channels.is_empty() {
        return Err(ApiError::BadRequest("cs lists no channels".to_owned()));
    }
    Ok(channels)
}

fn binary(bytes: bytes::Bytes, shape: &str, dtype: Dtype, level: u32) -> Response {
    let length = bytes.len();
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/octet-stream")
        .header(CONTENT_LENGTH, length)
        .header(SHAPE_HEADER, shape)
        .header(DTYPE_HEADER, dtype_name(dtype))
        .header(LEVEL_HEADER, level)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| {
            axum::response::IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR)
        })
}

fn header_number(value: u64) -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(&value.to_string())
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("0"))
}

fn dtype_name(dtype: Dtype) -> &'static str {
    match dtype {
        Dtype::U8 => "u8",
        Dtype::U16 => "u16",
        Dtype::U32 => "u32",
    }
}
