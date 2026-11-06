package com.clong.view;

import javax.swing.*;
import java.awt.*;

public class PicPanel extends JPanel {
    private Image image;

    public void setImage(Image image) {
        this.image = image;
    }

    @Override
    protected void paintComponent(Graphics g) {
        g.drawImage(image, 0, 0, this.getWidth(), this.getHeight(), null);
    }

}
