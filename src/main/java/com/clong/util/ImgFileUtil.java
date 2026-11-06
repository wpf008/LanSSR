package com.clong.util;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;

public class ImgFileUtil {

    /**
     * 直接读取文件为字节数组
     */
    public static byte[] readFile(String path) {
        try {
            return Files.readAllBytes(Paths.get(path));
        } catch (IOException e) {
            e.printStackTrace();
        }
        return null;
    }

    /**
     * 将字节数组写入文件
     */
    public static void writeToFile(byte[] data, String path) {
        File file = new File(path);
        try (OutputStream outputStream = new FileOutputStream(file)) {
            if (!file.exists()) {
                file.createNewFile();
            }
            outputStream.write(data);
        } catch (IOException e) {
            e.printStackTrace();
        }
    }

    /**
     * 计算字节数组的 MD5 哈希
     */
    public static String getMD5Hash(byte[] input) {
        try {
            MessageDigest md = MessageDigest.getInstance("MD5");
            byte[] digest = md.digest(input);
            StringBuilder sb = new StringBuilder();
            for (byte b : digest) {
                sb.append(String.format("%02x", b & 0xff));
            }
            return sb.toString();
        } catch (NoSuchAlgorithmException e) {
            throw new RuntimeException("MD5 not supported", e);
        }
    }

    /**
     * 将文件大小（字节）转换为人类可读的格式
     *
     * @param length 文件大小，单位为字节
     * @return 人类可读的文件大小字符串
     */
    public static String formatFileSize(long length) {
        // 定义单位数组
        String[] units = {"B", "KB", "MB", "GB", "TB"};
        int unitIndex = 0;
        double size = (double) length;

        // 每当 size >= 1024 并且还有更大的单位时，就除以 1024 并切换到下一个单位
        while (size >= 1024 && unitIndex < units.length - 1) {
            size /= 1024;
            unitIndex++;
        }

        return String.format("%.2f %s", size, units[unitIndex]);
    }
}