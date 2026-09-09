//! A minimal ONNX model, emitted as protobuf bytes with no dependencies.
//!
//! The spike needs *a* model to create a session with, and it must not depend
//! on a download: the thing under test is the dynamic load, and a network
//! failure must not read as a dylib failure. So the model is built here -
//! a single `Relu` over a `1x4` float tensor, opset 17, IR version 8.

fn varint(out: &mut Vec<u8>, mut v: u64) {
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

fn tag(out: &mut Vec<u8>, field: u32, wire: u32) {
    varint(out, ((field as u64) << 3) | wire as u64);
}

/// A varint field (wire type 0).
fn int_field(out: &mut Vec<u8>, field: u32, value: i64) {
    tag(out, field, 0);
    varint(out, value as u64);
}

/// A length-delimited field (wire type 2): strings and nested messages alike.
fn bytes_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    tag(out, field, 2);
    varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// `TypeProto` for a float tensor of the given static shape.
fn float_tensor_type(shape: &[i64]) -> Vec<u8> {
    let mut tensor_shape = Vec::new();
    for &d in shape {
        let mut dim = Vec::new();
        int_field(&mut dim, 1, d); // dim_value
        bytes_field(&mut tensor_shape, 1, &dim); // TensorShapeProto.dim
    }

    let mut tensor = Vec::new();
    int_field(&mut tensor, 1, 1); // elem_type = FLOAT
    bytes_field(&mut tensor, 2, &tensor_shape); // shape

    let mut ty = Vec::new();
    bytes_field(&mut ty, 1, &tensor); // TypeProto.tensor_type
    ty
}

fn value_info(name: &str, shape: &[i64]) -> Vec<u8> {
    let mut vi = Vec::new();
    bytes_field(&mut vi, 1, name.as_bytes()); // name
    bytes_field(&mut vi, 2, &float_tensor_type(shape)); // type
    vi
}

/// `Relu(x) -> y` over `1x4` floats. Small enough to inline, real enough that
/// creating a session exercises graph resolution, the allocator and the EP.
pub fn relu_1x4() -> Vec<u8> {
    let mut node = Vec::new();
    bytes_field(&mut node, 1, b"x"); // input
    bytes_field(&mut node, 2, b"y"); // output
    bytes_field(&mut node, 3, b"relu0"); // name
    bytes_field(&mut node, 4, b"Relu"); // op_type

    let mut graph = Vec::new();
    bytes_field(&mut graph, 1, &node); // node
    bytes_field(&mut graph, 2, b"spike"); // name
    bytes_field(&mut graph, 11, &value_info("x", &[1, 4])); // input
    bytes_field(&mut graph, 12, &value_info("y", &[1, 4])); // output

    let mut opset = Vec::new();
    bytes_field(&mut opset, 1, b""); // domain: ai.onnx
    int_field(&mut opset, 2, 17); // version

    let mut model = Vec::new();
    int_field(&mut model, 1, 8); // ir_version 8 == opset 17
    bytes_field(&mut model, 2, b"manga-cleaner-spike"); // producer_name
    bytes_field(&mut model, 7, &graph); // graph
    bytes_field(&mut model, 8, &opset); // opset_import
    model
}
