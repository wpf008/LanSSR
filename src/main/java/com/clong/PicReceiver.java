package com.clong;

import com.clong.data.Position;
import com.clong.thread.RPaserThread;
import com.clong.util.ImgFileUtil;
import com.clong.util.ImgUtil;

import java.awt.*;
import java.awt.datatransfer.Clipboard;
import java.awt.datatransfer.StringSelection;
import java.awt.image.BufferedImage;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class PicReceiver {
    private static Robot robot = null;

    public static int total = 0;

    public static String fileName;

    // 当前传输是否为文本模式（'t'信息帧）
    public static boolean isText = false;

    // 存储帧号对应的字节数据
    public static ConcurrentHashMap<Integer, byte[]> dataMap = new ConcurrentHashMap<>();

    public static ExecutorService threadPool = Executors.newFixedThreadPool(8);

    public static void main(String[] args) {
        System.out.println("寻找发送端");
        PicTransmitter.loadConfig();
        ImgUtil.waterMask = PicTransmitter.picConf.isWaterMask();
        Position position = null;
        while (position == null) {
            position = ImgUtil.findPosition();
            try {
                Thread.currentThread().sleep(1000L);
            } catch (InterruptedException e) {
                throw new RuntimeException(e);
            }
        }
        System.out.println("找到发送端：" + position);
        BufferedImage screen = ImgUtil.getScreenCapture(position.getRectangle());
//        Graphics graphics = screen.getGraphics();
//        graphics.setColor(Color.RED);
        PicTransmitter.cellInfoList.clear();
        boolean waterTag = false;
        for (int y = 0; y < position.getRectangle().height; y += position.getCellSize()) {
            for (int x = 0; x < position.getRectangle().width; x += position.getCellSize()) {
                Rectangle rect = new Rectangle(x, y, position.getCellSize(), position.getCellSize());
                PicTransmitter.cellInfoList.add(rect);
            }
        }

        System.out.println("数据格子数量：" + PicTransmitter.cellInfoList.size());
        transfer(position);
    }

    private static void transfer(Position position) {
        String lastSimple = null;
        long lastTime = System.currentTimeMillis();
        boolean hasPrint = false;
        while (total == 0 || total > dataMap.size()) {
            BufferedImage screen = ImgUtil.getScreenCapture(position.getRectangle());
            String msg = ImgUtil.simpleMsg(PicTransmitter.cellInfoList, screen);
            if (msg.equals(lastSimple)) {
                if (!hasPrint && System.currentTimeMillis() - lastTime > 3000) {
                    if (dataMap.size() > 10) {
                        hasPrint = true;
                        int count = 0;
                        boolean isFirst = true;
                        for (int i = 1; i <= total; i++) {
                            if (!dataMap.containsKey(i)) {
                                if (count >= 100) {
                                    break;
                                }
                                count++;
                                if (isFirst) {
                                    System.out.print(i);
                                    isFirst = false;
                                } else {
                                    System.out.print("," + i);
                                }
                            }
                        }
                        System.out.println();
                    }
                    lastTime = System.currentTimeMillis();
                }
                continue;
            }
            hasPrint = false;
            lastTime = System.currentTimeMillis();
            lastSimple = msg;
            RPaserThread thread = new RPaserThread();
            thread.setScreen(screen);
            threadPool.execute(thread);
//            msg = ImgFileUtil.hex2Str(msg);
        }

        threadPool.shutdown();

        // 合并所有帧数据
        ByteArrayOutputStream baos = new ByteArrayOutputStream();
        for (int i = 1; i <= total; i++) {
            byte[] chunk = dataMap.get(i);
            if (chunk != null) {
                baos.write(chunk, 0, chunk.length);
            }
        }
        byte[] allData = baos.toByteArray();
        if (isText) {
            // 文本模式：UTF-8解码后写入剪贴板
            String text = new String(allData, StandardCharsets.UTF_8);
            Clipboard clipboard = Toolkit.getDefaultToolkit().getSystemClipboard();
            clipboard.setContents(new StringSelection(text), null);
            System.out.println("收到文本，已写入剪贴板，文本长度：" + text.length());
        } else {
            ImgFileUtil.writeToFile(allData, fileName);
            System.out.println("收到文件：" + fileName);
        }

        while (!threadPool.isTerminated()) {
            try {
                Thread.currentThread().sleep(500L);
            } catch (InterruptedException e) {
                throw new RuntimeException(e);
            }
        }
        System.out.println("结束");
    }


}
