package com.clong.data;

import java.awt.*;

public class Position {
    private Rectangle rectangle;

    private int cellSize;

    public Rectangle getRectangle() {
        return rectangle;
    }

    public void setRectangle(Rectangle rectangle) {
        this.rectangle = rectangle;
    }

    public int getCellSize() {
        return cellSize;
    }

    public void setCellSize(int cellSize) {
        this.cellSize = cellSize;
    }

    @Override
    public String toString() {
        return "Position{" +
                "rectangle=" + rectangle +
                ", cellSize=" + cellSize +
                '}';
    }
}
