use mux_core::{RequestId, init_test_tracing};
use mux_protocol::{
    ClientMessage, PingRequest, decode_client_message, encode_client_message, read_frame,
    write_frame,
};

#[tokio::test]
async fn ping_round_trips_through_codec_and_frame() {
    init_test_tracing();

    let request = ClientMessage::Ping(PingRequest {
        request_id: RequestId(7),
        payload: "phase0".to_owned(),
    });
    let encoded = encode_client_message(&request).expect("encode request");
    let (mut writer, mut reader) = tokio::io::duplex(256);

    let write_task = tokio::spawn(async move {
        write_frame(&mut writer, &encoded)
            .await
            .expect("write frame");
    });

    let frame = read_frame(&mut reader)
        .await
        .expect("read frame")
        .expect("frame payload");
    write_task.await.expect("writer task joins");

    let decoded = decode_client_message(&frame).expect("decode request");
    assert_eq!(decoded, request);
}
