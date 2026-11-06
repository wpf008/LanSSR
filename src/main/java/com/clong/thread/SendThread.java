package com.clong.thread;

import com.clong.PicTransmitter;
import com.clong.util.ImgFileUtil;

import javax.swing.*;
import java.io.File;
import java.io.UnsupportedEncodingException;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * 发送端发送数据 - 二进制编码版本
 * 每个格子存储 4 bit，每个字节需要 2 个格子
 */
public class SendThread extends Thread {
    private static final Pattern pattern = Pattern.compile("^[0-9,-]+$");
    private String filePath;

    // 文本模式：true时发送的是剪贴板文本(UTF-8编码后的字节)
    private boolean textMode = false;

    private byte[] textData;

    public void setFilePath(String filePath) {
        this.filePath = filePath;
    }

    public void setTextData(byte[] textData) {
        this.textMode = true;
        this.textData = textData;
    }

    public void run() {
        long start = System.currentTimeMillis();
        sendData();
        long cost = System.currentTimeMillis() - start;
        String resend = PicTransmitter.lineField.getText().trim();
        if (resend.isEmpty()) {
            long length = textMode ? textData.length : new File(filePath).length();
            String msg = textMode ? "文本发送完成，大小：%s，耗时%d秒" : "发送完成，文件大小：%s，耗时%d秒";
            System.out.printf((msg) + "%n", ImgFileUtil.formatFileSize(length), cost / 1000);
        }
    }

    private void sendData() {
        String resend = PicTransmitter.lineField.getText().trim();
        final Set<Integer> lineSet = new HashSet<>();
        if (!resend.isEmpty()) {
            Matcher matcher = pattern.matcher(resend);
            if (!matcher.find()) {
                JOptionPane.showMessageDialog(null, "重发信息格式异常");
                return;
            }
            String[] nums = resend.split(",");
            for (String n : nums) {
                if (n.length() == 0) {
                    continue;
                }
                if (n.contains("-")) {
                    String[] nu = n.split("-");
                    if (nu.length == 2) {
                        String s0 = nu[0];
                        String s1 = nu[1];
                        for (int t = Integer.parseInt(s0); t <= Integer.parseInt(s1); t++) {
                            lineSet.add(t);
                        }
                    }
                } else {
                    lineSet.add(Integer.parseInt(n));
                }
            }
        }

        // 获取数据源：文本模式直接使用UTF-8字节，否则读取文件
        byte[] fileBytes;
        String name;
        if (textMode) {
            fileBytes = textData;
            name = "";
        } else {
            fileBytes = ImgFileUtil.readFile(filePath);
            if (fileBytes == null) {
                JOptionPane.showMessageDialog(null, "文件读取失败");
                return;
            }
            name = new File(filePath).getName();
        }

        // 每个格子存 4 bit，每字节需要 2 个格子
        int totalCells = PicTransmitter.cellInfoList.size();
        // 帧头开销：帧类型(1) + 帧号(3) + MD5校验(2) = 6 字节 = 12 格子
        int headerCells = 12;
        int dataCells = totalCells - headerCells;
        int dataSize = dataCells / 2;  // 每帧有效数据字节数

        int page = fileBytes.length / dataSize;
        if (fileBytes.length % dataSize > 0) {
            page++;
        }

        // 先发总信息帧：类型('f'文件/'t'文本) + 总帧数(4字节) + 名称
        byte[] nameBytes;
        try {
            nameBytes = URLEncoder.encode(name, "utf-8").getBytes(StandardCharsets.UTF_8);
        } catch (UnsupportedEncodingException e) {
            nameBytes = name.getBytes(StandardCharsets.UTF_8);
        }
        byte[] infoData = new byte[5 + nameBytes.length];
        infoData[0] = (byte) (textMode ? 't' : 'f');
        infoData[1] = (byte) ((page >> 24) & 0xFF);
        infoData[2] = (byte) ((page >> 16) & 0xFF);
        infoData[3] = (byte) ((page >> 8) & 0xFF);
        infoData[4] = (byte) (page & 0xFF);
        System.arraycopy(nameBytes, 0, infoData, 5, nameBytes.length);
        PicTransmitter.drawBytes(infoData);

        if (PicTransmitter.isStop) {
            return;
        }
        try {
            Thread.sleep(1000L);
        } catch (InterruptedException e) {
            throw new RuntimeException(e);
        }
        if (PicTransmitter.isStop) {
            return;
        }

        // 发送每帧数据
        for (int i = 1; i <= page; i++) {
            if (PicTransmitter.isStop) {
                break;
            }

            if (lineSet.size() > 0 && !lineSet.contains(i)) {
                continue;
            }

            PicTransmitter.label.setText(i + "/" + page);
            
            int dataStart = (i - 1) * dataSize;
            int dataEnd = Math.min(i * dataSize, fileBytes.length);
            byte[] chunkData = Arrays.copyOfRange(fileBytes, dataStart, dataEnd);

            // 构建帧：帧号(3字节) + MD5(2字节) + 数据
            byte[] md5Hash = ImgFileUtil.getMD5Hash(chunkData).getBytes(StandardCharsets.UTF_8);
            byte[] frame = new byte[5 + chunkData.length];
            frame[0] = (byte) ((i >> 16) & 0xFF);
            frame[1] = (byte) ((i >> 8) & 0xFF);
            frame[2] = (byte) (i & 0xFF);
            // MD5 后 2 字节 (4个十六进制字符转2字节)
            frame[3] = (byte) ((hexCharToVal(md5Hash[md5Hash.length - 4]) << 4) | hexCharToVal(md5Hash[md5Hash.length - 3]));
            frame[4] = (byte) ((hexCharToVal(md5Hash[md5Hash.length - 2]) << 4) | hexCharToVal(md5Hash[md5Hash.length - 1]));
            System.arraycopy(chunkData, 0, frame, 5, chunkData.length);

            PicTransmitter.drawBytes(frame);
            try {
                if (!resend.isEmpty()) {
                    Thread.sleep(200L);
                } else {
                    Thread.sleep(PicTransmitter.picConf.getInterval());
                }
            } catch (InterruptedException e) {
                throw new RuntimeException(e);
            }
        }
    }

    private int hexCharToVal(byte c) {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return 0;
    }
}
