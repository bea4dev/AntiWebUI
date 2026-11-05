// Prints where the fields `LAYOUTS` in src/ffi.rs asks for are, for the FFmpeg whose headers this is
// compiled with. Nothing of those headers goes anywhere but into these numbers.
//
//   cc tools/offsets.c -I<include dir of FFmpeg> -o offsets && ./offsets
#include <stdio.h>
#include <stddef.h>
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/frame.h>
#include <libavutil/hwcontext_drm.h>
#define P(t,f) printf("%-22s %-22s %5zu\n", #t, #f, offsetof(t,f))
int main(void){
printf("avcodec %d avformat %d avutil %d\n", LIBAVCODEC_VERSION_MAJOR, LIBAVFORMAT_VERSION_MAJOR, LIBAVUTIL_VERSION_MAJOR);
P(AVFormatContext,nb_streams);P(AVFormatContext,interrupt_callback);P(AVFormatContext,streams);P(AVFormatContext,duration);P(AVFormatContext,start_time);
P(AVStream,index);P(AVStream,codecpar);P(AVStream,time_base);P(AVStream,duration);P(AVStream,start_time);
P(AVCodecParameters,codec_type);P(AVCodecParameters,codec_id);P(AVCodecParameters,coded_side_data);P(AVCodecParameters,nb_coded_side_data);P(AVCodecParameters,width);P(AVCodecParameters,height);
P(AVCodecContext,opaque);P(AVCodecContext,get_format);P(AVCodecContext,hw_device_ctx);P(AVCodecContext,width);P(AVCodecContext,height);P(AVCodecContext,pkt_timebase);
P(AVFrame,data);P(AVFrame,linesize);P(AVFrame,extended_data);P(AVFrame,width);P(AVFrame,height);P(AVFrame,nb_samples);P(AVFrame,format);P(AVFrame,pts);P(AVFrame,best_effort_timestamp);P(AVFrame,sample_rate);P(AVFrame,ch_layout);P(AVFrame,color_range);P(AVFrame,color_primaries);P(AVFrame,color_trc);P(AVFrame,colorspace);P(AVFrame,hw_frames_ctx);P(AVFrame,sample_aspect_ratio);P(AVFrame,duration);P(AVFrame,crop_top);P(AVFrame,crop_bottom);P(AVFrame,crop_left);P(AVFrame,crop_right);
P(AVPacket,stream_index);P(AVPacket,pts);P(AVPacketSideData,data);P(AVPacketSideData,size);P(AVPacketSideData,type);
printf("sizes AVChannelLayout %zu AVPacketSideData %zu AVRational %zu\n",sizeof(AVChannelLayout),sizeof(AVPacketSideData),sizeof(AVRational));
printf("enum SIDE_DATA_DISPLAYMATRIX %d AVMEDIA_VIDEO %d AVMEDIA_AUDIO %d HWDEVICE_VAAPI %d HWDEVICE_DRM %d AV_HWFRAME_MAP_READ %d\n",AV_PKT_DATA_DISPLAYMATRIX,AVMEDIA_TYPE_VIDEO,AVMEDIA_TYPE_AUDIO,AV_HWDEVICE_TYPE_VAAPI,AV_HWDEVICE_TYPE_DRM,AV_HWFRAME_MAP_READ);
printf("spaces BT709 %d BT470BG %d SMPTE170M %d BT2020_NCL %d range MPEG %d JPEG %d trc PQ %d HLG %d\n",AVCOL_SPC_BT709,AVCOL_SPC_BT470BG,AVCOL_SPC_SMPTE170M,AVCOL_SPC_BT2020_NCL,AVCOL_RANGE_MPEG,AVCOL_RANGE_JPEG,AVCOL_TRC_SMPTE2084,AVCOL_TRC_ARIB_STD_B67);
printf("DRM desc size %zu layer size %zu object size %zu plane size %zu\n",sizeof(AVDRMFrameDescriptor),sizeof(AVDRMLayerDescriptor),sizeof(AVDRMObjectDescriptor),sizeof(AVDRMPlaneDescriptor));
P(AVDRMFrameDescriptor,nb_objects);P(AVDRMFrameDescriptor,objects);P(AVDRMFrameDescriptor,nb_layers);P(AVDRMFrameDescriptor,layers);
return 0;}
