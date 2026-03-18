#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libswresample/swresample.h>
#include <libavutil/channel_layout.h>
#include <emscripten.h>

extern const AVCodec ff_ac3_decoder;
extern const AVCodec ff_eac3_decoder;
extern const AVCodec ff_truehd_decoder;

static AVCodecContext *dec_ctx = NULL;
static AVFrame *frame = NULL;
static AVPacket *pkt = NULL;
static SwrContext *swr = NULL;

static float *audio_buffer = NULL;
#define MAX_SAMPLES 192000 

static int target_sample_rate = 48000;

// 1. INITIALIZATION
EMSCRIPTEN_KEEPALIVE
int init_audio(int codec_id, int sample_rate, int channels, uint8_t *extradata, int extradata_size) {
    const AVCodec *codec = NULL;

    if (codec_id == 86019) codec = &ff_ac3_decoder;
    else if (codec_id == 86057) codec = &ff_eac3_decoder;
    else if (codec_id == 86060) codec = &ff_truehd_decoder;
    else codec = avcodec_find_decoder(codec_id);

    if (!codec) return -1;

    dec_ctx = avcodec_alloc_context3(codec);
    
    // Apply dynamic sample rate and channels
    target_sample_rate = sample_rate > 0 ? sample_rate : 48000;
    dec_ctx->sample_rate = target_sample_rate;
    av_channel_layout_default(&dec_ctx->ch_layout, channels > 0 ? channels : 2);

    // Apply ExtraData (Required for many modern audio formats)
    if (extradata_size > 0 && extradata != NULL) {
        dec_ctx->extradata = (uint8_t *)malloc(extradata_size + AV_INPUT_BUFFER_PADDING_SIZE);
        memset(dec_ctx->extradata + extradata_size, 0, AV_INPUT_BUFFER_PADDING_SIZE); // Safety padding
        memcpy(dec_ctx->extradata, extradata, extradata_size);
        dec_ctx->extradata_size = extradata_size;
    }

    if (avcodec_open2(dec_ctx, codec, NULL) < 0) return -2;

    frame = av_frame_alloc();
    pkt = av_packet_alloc();
    
    audio_buffer = (float *)malloc(MAX_SAMPLES * 2 * sizeof(float));

    return 0;
}

// 2. DECODING
EMSCRIPTEN_KEEPALIVE
int decode_audio(uint8_t *data_ptr, int data_size) {
    if (!dec_ctx) return -99;

    av_packet_unref(pkt);
    pkt->data = data_ptr;
    pkt->size = data_size;

    if (avcodec_send_packet(dec_ctx, pkt) < 0) return -1;

    int ret = avcodec_receive_frame(dec_ctx, frame);
    if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) return 0;
    if (ret < 0) return -2;

    // 3. RESAMPLING
    if (!swr) {
        AVChannelLayout out_layout;
        av_channel_layout_default(&out_layout, 2);

        swr_alloc_set_opts2(&swr, 
                            &out_layout, 
                            AV_SAMPLE_FMT_FLTP, 
                            target_sample_rate,
                            &frame->ch_layout, 
                            dec_ctx->sample_fmt, 
                            frame->sample_rate, 
                            0, NULL);
        swr_init(swr);
    }

    // Fixed array initialization for emscripten
    uint8_t *out_ptrs[2];
    out_ptrs[0] = (uint8_t *)audio_buffer;
    out_ptrs[1] = (uint8_t *)(audio_buffer + MAX_SAMPLES);

    return swr_convert(swr, out_ptrs, MAX_SAMPLES, (const uint8_t **)frame->data, frame->nb_samples);
}

// 4. MEMORY HANDOFF
EMSCRIPTEN_KEEPALIVE
float* get_audio_ptr() {
    return audio_buffer;
}

// 5. CLEANUP
EMSCRIPTEN_KEEPALIVE
void free_audio() {
    if (swr) swr_free(&swr);
    if (dec_ctx) avcodec_free_context(&dec_ctx);
    if (frame) av_frame_free(&frame);
    if (pkt) av_packet_free(&pkt);
    if (audio_buffer) {
        free(audio_buffer);
        audio_buffer = NULL;
    }
}