package com.clong.thread;

import com.clong.PicReceiver;
import com.clong.PicTransmitter;
import com.clong.util.ImgFileUtil;
import com.clong.util.ImgUtil;

import java.awt.image.BufferedImage;
import java.io.UnsupportedEncodingException;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;

/**
 * 接收端解析截图获取内容 - 二进制编码版本
 * 每张截图一个线程
 */
public class RPaserThread extends Thread {
    private BufferedImage screen;

    public BufferedImage getScreen() {
        return screen;
    }

    public void setScreen(BufferedImage screen) {
        this.screen = screen;
    }

    public void run() {
        byte[] frameData = ImgUtil.parseBytes(PicTransmitter.cellInfoList, screen);
        if (frameData == null || frameData.length < 5) {
            return;
        }

        // 判断帧类型：'f'=文件信息帧，'t'=文本信息帧
        if (frameData[0] == (byte) 'f' || frameData[0] == (byte) 't') {
            // 信息帧: 'f'/'t' + 总帧数(4字节) + 名称
            PicReceiver.isText = frameData[0] == (byte) 't';
            int total = ((frameData[1] & 0xFF) << 24) | ((frameData[2] & 0xFF) << 16) | 
                        ((frameData[3] & 0xFF) << 8) | (frameData[4] & 0xFF);
            byte[] nameBytes = new byte[frameData.length - 5];
            System.arraycopy(frameData, 5, nameBytes, 0, nameBytes.length);
            String fileName = null;
            try {
                fileName = URLDecoder.decode(new String(nameBytes, StandardCharsets.UTF_8), "UTF-8");
            } catch (UnsupportedEncodingException e) {
                throw new RuntimeException(e);
            }
            PicReceiver.total = total;
            PicReceiver.fileName = fileName;
            System.out.println((PicReceiver.isText ? "文本" : "文件") + "总帧数：" + total + "，名称：" + fileName);
            return;
        }

        // 数据帧: 帧号(3字节) + MD5(2字节) + 数据
        int frameNum = ((frameData[0] & 0xFF) << 16) | ((frameData[1] & 0xFF) << 8) | (frameData[2] & 0xFF);
        if (frameNum <= 0 || frameNum >= 986895) {
            return;
        }

        if (PicReceiver.dataMap.containsKey(frameNum)) {
            return;
        }

        byte[] md5Received = new byte[2];
        md5Received[0] = frameData[3];
        md5Received[1] = frameData[4];

        byte[] chunkData = new byte[frameData.length - 5];
        System.arraycopy(frameData, 5, chunkData, 0, chunkData.length);

        // 校验 MD5
        String md5Hash = ImgFileUtil.getMD5Hash(chunkData);
        byte[] md5Expected = new byte[2];
        md5Expected[0] = (byte) ((hexCharToVal(md5Hash.charAt(md5Hash.length() - 4)) << 4) | hexCharToVal(md5Hash.charAt(md5Hash.length() - 3)));
        md5Expected[1] = (byte) ((hexCharToVal(md5Hash.charAt(md5Hash.length() - 2)) << 4) | hexCharToVal(md5Hash.charAt(md5Hash.length() - 1)));

        if (md5Received[0] == md5Expected[0] && md5Received[1] == md5Expected[1]) {
            PicReceiver.dataMap.put(frameNum, chunkData);
        } else {
            System.out.println(frameNum + " MD5校验失败");
        }
    }

    private int hexCharToVal(char c) {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return 0;
    }
}
