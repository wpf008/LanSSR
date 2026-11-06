package com.clong.thread;

import com.clong.PicTransmitter;
import com.clong.data.Position;
import com.clong.util.ImgUtil;

import java.awt.*;
import java.awt.image.BufferedImage;

/**
 * 寻找发送端界面
 */
public class FindLocationThread extends Thread {
    private Position position;

    public void run() {
        try {
            Thread.currentThread().sleep(1000L);
        } catch (InterruptedException e) {
            throw new RuntimeException(e);
        }
        ImgUtil.defRect = null;
        position = ImgUtil.findPosition();
        if (position == null) {
            throw new RuntimeException("未找到发送端界面");
        } else {
            System.out.println("找到界面：" + position);
        }
        BufferedImage screen = ImgUtil.getScreenCapture(position.getRectangle());
        PicTransmitter.cellInfoList.clear();
        boolean waterTag = false;
        Graphics graphics = PicTransmitter.image.getGraphics();
        graphics.setColor(Color.BLUE);
        for (int y = 0; y < position.getRectangle().height; y += position.getCellSize()) {
            for (int x = 0; x < position.getRectangle().width; x += position.getCellSize()) {
                Rectangle rect = new Rectangle(x, y, position.getCellSize(), position.getCellSize());
                PicTransmitter.cellInfoList.add(rect);
            }
        }
        System.out.println("数据格子数量：" + PicTransmitter.cellInfoList.size());
    }
}
