package com.clong.data;

public class PicConf {
    private int width;

    private int height;

    private int cellSize;

    private long interval;

    private boolean waterMask;

    public int getWidth() {
        return width;
    }

    public void setWidth(int width) {
        this.width = width;
    }

    public int getHeight() {
        return height;
    }

    public void setHeight(int height) {
        this.height = height;
    }

    public int getCellSize() {
        return cellSize;
    }

    public void setCellSize(int cellSize) {
        this.cellSize = cellSize;
    }

    public long getInterval() {
        return interval;
    }

    public void setInterval(long interval) {
        this.interval = interval;
    }

    public boolean isWaterMask() {
        return waterMask;
    }

    public void setWaterMask(boolean waterMask) {
        this.waterMask = waterMask;
    }
}
