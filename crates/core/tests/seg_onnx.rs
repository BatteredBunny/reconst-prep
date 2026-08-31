use reconst_prep_core::mask::MaskClass;
use reconst_prep_core::seg::{SegClassParams, SegConfig, SegModel};

// Varints and length-delimited fields only; field numbers from onnx.proto3.

fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn field_varint(field: u32, v: u64, out: &mut Vec<u8>) {
    varint((field as u64) << 3, out);
    varint(v, out);
}

fn field_bytes(field: u32, bytes: &[u8], out: &mut Vec<u8>) {
    varint(((field as u64) << 3) | 2, out);
    varint(bytes.len() as u64, out);
    out.extend_from_slice(bytes);
}

fn msg(build: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut v = Vec::new();
    build(&mut v);
    v
}

/// TensorShapeProto with fixed dimensions.
fn shape(dims: &[i64]) -> Vec<u8> {
    msg(|s| {
        for &d in dims {
            // TensorShapeProto.dim (1) -> Dimension.dim_value (1)
            field_bytes(1, &msg(|dim| field_varint(1, d as u64, dim)), s);
        }
    })
}

/// TypeProto holding a float tensor of the given shape.
fn float_tensor_type(dims: &[i64]) -> Vec<u8> {
    msg(|t| {
        // TypeProto.tensor_type (1)
        field_bytes(
            1,
            &msg(|tt| {
                field_varint(1, 1, tt); // elem_type = FLOAT
                field_bytes(2, &shape(dims), tt);
            }),
            t,
        );
    })
}

fn value_info(name: &str, dims: &[i64]) -> Vec<u8> {
    msg(|v| {
        field_bytes(1, name.as_bytes(), v);
        field_bytes(2, &float_tensor_type(dims), v);
    })
}

/// AttributeProto carrying a list of ints (type INTS = 7).
fn ints_attr(name: &str, values: &[i64]) -> Vec<u8> {
    msg(|a| {
        field_bytes(1, name.as_bytes(), a);
        for &v in values {
            field_varint(8, v as u64, a); // AttributeProto.ints
        }
        field_varint(20, 7, a); // AttributeProto.type = INTS
    })
}

/// AttributeProto carrying a single int (type INT = 2).
fn int_attr(name: &str, value: i64) -> Vec<u8> {
    msg(|a| {
        field_bytes(1, name.as_bytes(), a);
        field_varint(3, value as u64, a); // AttributeProto.i
        field_varint(20, 2, a); // AttributeProto.type = INT
    })
}

fn node(op_type: &str, inputs: &[&str], outputs: &[&str], attrs: &[Vec<u8>]) -> Vec<u8> {
    msg(|n| {
        for i in inputs {
            field_bytes(1, i.as_bytes(), n);
        }
        for o in outputs {
            field_bytes(2, o.as_bytes(), n);
        }
        field_bytes(3, op_type.as_bytes(), n); // name
        field_bytes(4, op_type.as_bytes(), n); // op_type
        for a in attrs {
            field_bytes(5, a, n);
        }
    })
}

/// A complete ModelProto with one node.
fn onnx_model(nodes: &[Vec<u8>], input: (&str, &[i64]), output: (&str, &[i64])) -> Vec<u8> {
    let graph = msg(|g| {
        for n in nodes {
            field_bytes(1, n, g);
        }
        field_bytes(2, b"test", g); // name
        field_bytes(11, &value_info(input.0, input.1), g);
        field_bytes(12, &value_info(output.0, output.1), g);
    });
    msg(|m| {
        field_varint(1, 8, m); // ir_version (ONNX 1.12)
        field_bytes(2, b"reconst-prep-test", m); // producer_name
        field_bytes(7, &graph, m); // graph
        // opset_import: default domain, version 13
        field_bytes(
            8,
            &msg(|o| {
                field_bytes(1, b"", o);
                field_varint(2, 13, o);
            }),
            m,
        );
    })
}

/// Removes itself on drop, so a failing assertion does not leave it behind.
struct TempModel(std::path::PathBuf);

impl std::ops::Deref for TempModel {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempModel {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_model(name: &str, bytes: &[u8]) -> TempModel {
    let path =
        std::env::temp_dir().join(format!("reconst-prep-{name}-{}.onnx", std::process::id()));
    std::fs::write(&path, bytes).expect("writing test model");
    TempModel(path)
}

/// The argmax over normalized channels is the dominant colour channel, so blue -> 2 and green -> 1.
fn identity_model() -> Vec<u8> {
    onnx_model(
        &[node("Identity", &["x"], &["y"], &[])],
        ("x", &[1, 3, 64, 64]),
        ("y", &[1, 3, 64, 64]),
    )
}

// --- test images ----------------------------------------------------------

/// Blue top half (class 2 = sky), green bottom half (class 1).
fn sky_over_green(w: u32, h: u32) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for _ in 0..w {
            if y < h / 2 {
                rgb.extend_from_slice(&[0, 0, 255]);
            } else {
                rgb.extend_from_slice(&[0, 255, 0]);
            }
        }
    }
    rgb
}

fn seg_config(width: u32, temporal_window: u32) -> SegConfig {
    SegConfig {
        width,
        // Dilation off: this is about which pixels the model claims.
        sky: Some(SegClassParams {
            class_id: 2,
            dilate: 0,
        }),
        people: Some(SegClassParams {
            class_id: 1,
            dilate: 0,
        }),
        temporal_window,
    }
}

// --- tests ----------------------------------------------------------------

#[test]
fn identity_model_maps_channels_to_classes() {
    let path = write_model("identity", &identity_model());
    let cfg = seg_config(64, 1);
    let model = SegModel::load(&path, &cfg, 64, 64).expect("loading the hand-built model");
    assert_eq!(model.size(), (64, 64));
    assert_eq!(model.n_classes(), 3);

    let rgb = sky_over_green(64, 64);
    let labels = model.labels(&rgb, 64, 64).expect("inference");
    assert_eq!(labels.data[0], 2, "top row is blue, i.e. class 2");
    assert_eq!(labels.data[63 * 64], 1, "bottom row is green, i.e. class 1");

    let masks = model.masks(&labels);
    let sky = masks
        .iter()
        .find(|(c, _)| *c == MaskClass::Sky)
        .map(|(_, m)| m)
        .expect("a sky mask");
    let people = masks
        .iter()
        .find(|(c, _)| *c == MaskClass::People)
        .map(|(_, m)| m)
        .expect("a people mask");

    assert_eq!(sky.data[0], 0, "sky is masked out at the top");
    assert_eq!(sky.data[63 * 64], 255, "the green half is not sky");
    assert_eq!(people.data[0], 255, "the blue half is not a person");
    assert_eq!(people.data[63 * 64], 0, "class 1 is masked as people");
    assert!((sky.valid_fraction() - 0.5).abs() < 0.02);
    assert!((people.valid_fraction() - 0.5).abs() < 0.02);
}

#[test]
fn logits_at_a_lower_stride_are_upsampled_to_input_size() {
    // SegFormer-class nets emit logits at 1/4 resolution; MaxPool 2x2/2 is the smallest stand-in.
    let bytes = onnx_model(
        &[node(
            "MaxPool",
            &["x"],
            &["y"],
            &[
                ints_attr("kernel_shape", &[2, 2]),
                ints_attr("strides", &[2, 2]),
            ],
        )],
        ("x", &[1, 3, 64, 64]),
        ("y", &[1, 3, 32, 32]),
    );
    let path = write_model("maxpool", &bytes);
    let cfg = seg_config(64, 1);
    let model = SegModel::load(&path, &cfg, 64, 64).expect("loading the strided model");

    let rgb = sky_over_green(64, 64);
    let labels = model.labels(&rgb, 64, 64).expect("inference");
    assert_eq!(
        (labels.w, labels.h),
        (64, 64),
        "labels must be upsampled back to the inference input size"
    );
    assert_eq!(labels.data.len(), 64 * 64);
    assert_eq!(labels.data[0], 2);
    assert_eq!(labels.data[63 * 64], 1);
}

#[test]
fn a_wrong_class_id_is_rejected_at_load_time() {
    // Better to fail in the first second than to write 5 000 empty masks.
    let path = write_model("badclass", &identity_model());
    let cfg = SegConfig {
        sky: Some(SegClassParams {
            class_id: 99,
            dilate: 0,
        }),
        people: None,
        ..seg_config(64, 1)
    };
    let err = SegModel::load(&path, &cfg, 64, 64).expect_err("class 99 of 3 must be rejected");
    let msg = format!("{err:#}");
    assert!(msg.contains("3 classes"), "unhelpful message: {msg}");
}

#[test]
fn a_model_that_is_not_a_segmentation_net_is_rejected() {
    // ReduceMean over the spatial axes really does collapse, unlike the rank-polymorphic Identity.
    let bytes = onnx_model(
        &[node(
            "ReduceMean",
            &["x"],
            &["y"],
            &[ints_attr("axes", &[2, 3]), int_attr("keepdims", 0)],
        )],
        ("x", &[1, 3, 64, 64]),
        ("y", &[1, 3]),
    );
    let path = write_model("not-seg", &bytes);
    let err = SegModel::load(&path, &seg_config(64, 1), 64, 64)
        .expect_err("a rank-2 model must be rejected");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("segmentation model") || msg.contains("1xCxHxW"),
        "unhelpful message: {msg}"
    );
}
