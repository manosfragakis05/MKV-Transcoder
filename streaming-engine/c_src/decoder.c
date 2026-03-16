#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <libavcodec/avcodec.h>
#include <libswresample/swresample.h>
#include <libavutil/channel_layout.h>
#include <libavutil/opt.h>

extern const AVCodec ff_eac3_decoder;
extern const AVCodec ff_ac3_decoder;
extern const AVCodec ff_truehd_decoder;

static AVCodecContext *codec_ctx = NULL;
static AVFrame *decoded_frame = NULL;
static AVPacket *pkt = NULL;
static SwrContext *swr_ctx = NULL;
static float *pcm_out_buffer = NULL;
static int max_pcm_out_samples = 48000 * 2;

int init_audio_decoder(int codec_id, int sample_rate, int channels, const uint8_t *extradata, int extradata_size)
{
    const AVCodec *codec = NULL;

    if (codec_id == 86057)
        codec = (const AVCodec *)&ff_eac3_decoder;
    else if (codec_id == 86019)
        codec = (const AVCodec *)&ff_ac3_decoder;
    else if (codec_id == 86060)
        codec = (const AVCodec *)&ff_truehd_decoder;
    else
        codec = avcodec_find_decoder((enum AVCodecID)codec_id);

    if (!codec)
        return -1;

    codec_ctx = avcodec_alloc_context3(codec);
    if (!codec_ctx)
        return -2;

    codec_ctx->sample_rate = sample_rate > 0 ? sample_rate : 48000;
    codec_ctx->thread_count = 1;
    av_channel_layout_default(&codec_ctx->ch_layout, channels > 0 ? channels : 2);

    if (extradata_size > 0 && extradata != NULL)
    {
        codec_ctx->extradata = (uint8_t *)av_mallocz(extradata_size + 64);
        if (codec_ctx->extradata)
        {
            memcpy(codec_ctx->extradata, extradata, extradata_size);
            codec_ctx->extradata_size = extradata_size;
        }
    }

    if (avcodec_open2(codec_ctx, codec, NULL) < 0)
        return -3;

    decoded_frame = av_frame_alloc();
    pkt = av_packet_alloc();
    if (!pcm_out_buffer)
        pcm_out_buffer = (float *)malloc(max_pcm_out_samples * sizeof(float) * 2);

    return 0;
}

// 🔊 THE REAL DECODER (FFmpeg 7.0 SWR Fix)
int decode_audio_frame(const uint8_t *input_data, int input_size)
{
    if (!codec_ctx || !pkt || !decoded_frame)
        return -99;

    pkt->data = (uint8_t *)input_data;
    pkt->size = input_size;

    int ret = avcodec_send_packet(codec_ctx, pkt);
    if (ret < 0)
        return -4;

    ret = avcodec_receive_frame(codec_ctx, decoded_frame);
    if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF)
        return 0;
    if (ret < 0)
        return -5;

    // 🔥 THE NEW FFMPEG 7.0 RESAMPLER API 🔥
    if (!swr_ctx)
    {
        AVChannelLayout out_ch_layout;
        av_channel_layout_default(&out_ch_layout, 2); // Force WebAudio Stereo

        int ret_swr = swr_alloc_set_opts2(&swr_ctx,
                                          &out_ch_layout,             // Output: Stereo
                                          AV_SAMPLE_FMT_FLT,          // Output: Float32 for JS Web Audio
                                          48000,                      // Output: 48kHz
                                          &decoded_frame->ch_layout,  // Input: Whatever Dolby gave us (5.1, 7.1, etc)
                                          codec_ctx->sample_fmt,      // Input: Dolby Sample Format
                                          decoded_frame->sample_rate, // Input: Dolby Sample Rate
                                          0, NULL);

        if (ret_swr < 0 || !swr_ctx)
            return -6;

        if (swr_init(swr_ctx) < 0)
            return -7;
    }

    uint8_t *out_ptrs[1] = {(uint8_t *)pcm_out_buffer};
    int samples_converted = swr_convert(swr_ctx, out_ptrs, max_pcm_out_samples,
                                        (const uint8_t **)decoded_frame->data, decoded_frame->nb_samples);

    return samples_converted;
}

float *get_audio_buffer_ptr() { return pcm_out_buffer; }

void cleanup_audio_decoder()
{
    if (swr_ctx)
        swr_free(&swr_ctx);
    if (codec_ctx)
        avcodec_free_context(&codec_ctx);
    if (decoded_frame)
        av_frame_free(&decoded_frame);
    if (pkt)
        av_packet_free(&pkt);
    if (pcm_out_buffer)
    {
        free(pcm_out_buffer);
        pcm_out_buffer = NULL;
    }
}