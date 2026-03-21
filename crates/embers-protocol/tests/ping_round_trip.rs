use embers_core::{RequestId, init_test_tracing};
use embers_protocol::{
    ClientMessage, FrameType, PingRequest, RawFrame, decode_client_message, encode_client_message,
    read_frame, write_frame,
};

#[tokio::test]
async fn ping_round_trips_through_codec_and_frame() {
    init_test_tracing();

    let request = ClientMessage::Ping(PingRequest {
        request_id: RequestId(7),
        payload: "phase2".to_owned(),
    });
    let payload = encode_client_message(&request).expect("encode request");
    let frame = RawFrame::new(FrameType::Request, RequestId(7), payload);
    let (mut writer, mut reader) = tokio::io::duplex(256);

    let write_task = tokio::spawn(async move {
        write_frame(&mut writer, &frame).await.expect("write frame");
    });

    let received_frame = read_frame(&mut reader)
        .await
        .expect("read frame")
        .expect("frame payload");
    write_task.await.expect("writer task joins");

    assert_eq!(received_frame.frame_type, FrameType::Request);
    assert_eq!(received_frame.request_id, RequestId(7));

    let decoded = decode_client_message(&received_frame.payload).expect("decode request");
    assert_eq!(decoded, request);
}
