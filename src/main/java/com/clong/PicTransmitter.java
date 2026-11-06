package com.clong;

import com.clong.data.PicConf;
import com.clong.thread.FindLocationThread;
import com.clong.thread.SendThread;
import com.clong.view.PicPanel;

import javax.swing.*;
import java.awt.*;
import java.awt.datatransfer.Clipboard;
import java.awt.datatransfer.DataFlavor;
import java.awt.datatransfer.UnsupportedFlavorException;
import java.awt.dnd.DnDConstants;
import java.awt.dnd.DropTarget;
import java.awt.dnd.DropTargetAdapter;
import java.awt.dnd.DropTargetDropEvent;
import java.awt.image.BufferedImage;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Properties;

public class PicTransmitter {
    public static PicConf picConf;

    private static File file;

    public static BufferedImage image;

    public static boolean isStop = false;

    public static PicPanel panel = new PicPanel();

    public static JTextField lineField = new JTextField();

    // 存储所有格子信息
    public static List<Rectangle> cellInfoList = new ArrayList<>();

    public static JLabel label = new JLabel("0/0");

    public static void main(String[] args) {
        loadConfig();
        JFrame frame = initFrame();
        frame.setVisible(true);
    }

    /**
     * 直接从字节数组绘制格子
     * 每个字节拆成高低4位，分别存入两个格子
     */
    public static void drawBytes(byte[] data) {
        Graphics graphics = image.getGraphics();
        graphics.setColor(Color.RED);
        graphics.fillRect(0, 0, image.getWidth(), image.getHeight());

        int cellIndex = 0;
        for (byte b : data) {
            if (cellIndex >= cellInfoList.size() - 1) break;
            // 高4位
            int high = (b >> 4) & 0x0F;
            drawCell(graphics, cellIndex++, high);
            // 低4位
            int low = b & 0x0F;
            drawCell(graphics, cellIndex++, low);
        }
        panel.repaint();
    }

    private static void drawCell(Graphics g, int index, int value) {
        if (index >= cellInfoList.size()) return;
        Rectangle rect = cellInfoList.get(index);

        if (picConf.isWaterMask()) {
            // 有水印时：将格子分为4个小格子（左上、右上、左下、右下）
            // 每个小格子用黑白两种状态表示，4位可以表示0-15
            int halfWidth = rect.width / 2;
            int halfHeight = rect.height / 2;

            // 将value(0-15)转换为4位二进制
            // bit3(最高位)->左上, bit2->右上, bit1->左下, bit0(最低位)->右下
            boolean[] bits = new boolean[4];
            bits[0] = ((value >> 3) & 1) == 1;  // 左上
            bits[1] = ((value >> 2) & 1) == 1;  // 右上
            bits[2] = ((value >> 1) & 1) == 1;  // 左下
            bits[3] = (value & 1) == 1;         // 右下

            // 统计黑色格子的数量
            int blackCount = 0;
            for (boolean bit : bits) {
                if (bit) blackCount++;
            }

            // 根据黑白格子数量决定绘制策略：少的作为前景色
            boolean useWhiteAsForeground = blackCount > 2;
            Color backgroundColor = useWhiteAsForeground ? Color.BLACK : Color.WHITE;
            Color foregroundColor = useWhiteAsForeground ? Color.WHITE : Color.BLACK;

            // 先画背景
            g.setColor(backgroundColor);
            g.fillRect(rect.x, rect.y, rect.width, rect.height);

            // 定义4个小格子的位置
            int[][] positions = {
                    {rect.x, rect.y},                                          // 左上
                    {rect.x + halfWidth, rect.y},                             // 右上
                    {rect.x, rect.y + halfHeight},                            // 左下
                    {rect.x + halfWidth, rect.y + halfHeight}                 // 右下
            };

            // 只画需要的前景色格子
            g.setColor(foregroundColor);
            for (int i = 0; i < 4; i++) {
                boolean shouldDraw = useWhiteAsForeground ? !bits[i] : bits[i];
                if (shouldDraw) {
                    g.fillRect(positions[i][0], positions[i][1], halfWidth, halfHeight);
                }
            }
        } else {
            // 无水印时：取每档灰度中点着色（value*16+8），0-255全区间对称分布
            int gray = value * 16 + 8;  // 8-248
            g.setColor(new Color(gray, gray, gray));
            g.fillRect(rect.x, rect.y, rect.width, rect.height);
        }
    }

    private static JFrame initFrame() {
        JFrame frame = new JFrame("视力测试");
        frame.setAlwaysOnTop(true);
        frame.setResizable(false);
        frame.setLocation(0, 0);
        frame.setSize(picConf.getWidth() + 60, picConf.getHeight() + 155);
        frame.setDefaultCloseOperation(JFrame.EXIT_ON_CLOSE);
        Container contentPane = frame.getContentPane();
        JTextField field = new JTextField("");
        field.setEditable(false);
        DropTargetAdapter dropTargetAdapter = new DropTargetAdapter() {
            @Override
            public void drop(DropTargetDropEvent dtde) {
                dtde.acceptDrop(DnDConstants.ACTION_COPY_OR_MOVE);
                List<File> fileList = null;
                try {
                    fileList = (List<File>) dtde.getTransferable()
                            .getTransferData(DataFlavor.javaFileListFlavor);
                } catch (UnsupportedFlavorException | IOException e) {
                    throw new RuntimeException(e);
                }
                file = fileList.get(0);
                field.setText(fileList.get(0).getName());
            }
        };
        field.setBounds(2, 2, 100, 25);
        contentPane.setLayout(null);
        contentPane.add(field);
        label.setBounds(365, 2, 60, 25);
        contentPane.add(label);

        lineField.setBounds(10, 40, 200, 25);
        contentPane.add(lineField);
        JLabel tipLabel = new JLabel("补发帧数号，英文逗号分隔，单个数字或-分隔的两个数字");
        tipLabel.setFont(new Font("宋体", Font.PLAIN, 12));
        tipLabel.setBounds(220, 40, 500, 25);
        contentPane.add(tipLabel);

        JButton sendBtn = new JButton("发送");
        sendBtn.setBounds(105, 2, 60, 25);
        sendBtn.addActionListener(e -> {
            send();
        });

        JButton resetBtn = new JButton("重置");
        resetBtn.addActionListener(e -> {
            field.setText("");
            file = null;
            label.setText("0/0");
            reset();
        });
        resetBtn.setBounds(165, 2, 60, 25);
        JButton stopBtn = new JButton("停止");
        stopBtn.addActionListener(e -> {
            stop();
        });
        stopBtn.setBounds(225, 2, 70, 25);
        JButton textBtn = new JButton("文本");
        textBtn.addActionListener(e -> {
            sendText();
        });
        textBtn.setBounds(295, 2, 60, 25);
        contentPane.add(sendBtn);
        contentPane.add(resetBtn);
        contentPane.add(stopBtn);
        contentPane.add(textBtn);
        contentPane.setBackground(Color.WHITE);
        // 文件拖拽
        new DropTarget(contentPane, dropTargetAdapter);

        panel.setBounds(20, 85, picConf.getWidth(), picConf.getHeight());
        panel.setImage(image);
        contentPane.add(panel);
        reset();
        return frame;
    }

    private static void stop() {
        isStop = true;
    }

    private static void reset() {
        cellInfoList.clear();
        lineField.setText("");
        stop();
        // 初始化页面
        int cellSize = picConf.getCellSize();
        Graphics graphics = image.getGraphics();
        graphics.setColor(Color.black);
        graphics.fillRect(0, 0, picConf.getWidth(), picConf.getHeight());
        graphics.setColor(Color.WHITE);

        for (int i = 0; i < image.getHeight() / cellSize; i++) {
            for (int j = 0; j < image.getWidth() / cellSize; j++) {
                if ((i + j) % 2 == 1) {
                    graphics.fillRect(j * cellSize, i * cellSize, cellSize, cellSize);
                }
            }
        }

        panel.repaint();

        // 数据格子初始化
        FindLocationThread thread = new FindLocationThread();
        thread.start();
    }

    private static void send() {
        if (file == null) {
            JOptionPane.showMessageDialog(panel, "请先拖入文件");
            return;
        }
        isStop = false;
        SendThread sendThread = new SendThread();
        sendThread.setFilePath(file.getAbsolutePath());
        sendThread.start();
    }

    private static void sendText() {
        // 读取剪贴板文本
        String text;
        try {
            Clipboard clipboard = Toolkit.getDefaultToolkit().getSystemClipboard();
            text = (String) clipboard.getData(DataFlavor.stringFlavor);
        } catch (Exception e) {
            JOptionPane.showMessageDialog(panel, "剪贴板中没有可用的文本");
            return;
        }
        if (text == null || text.isEmpty()) {
            JOptionPane.showMessageDialog(panel, "剪贴板文本为空");
            return;
        }
        // 文本按UTF-8编码，按文件方式分帧发送
        isStop = false;
        SendThread sendThread = new SendThread();
        sendThread.setTextData(text.getBytes(StandardCharsets.UTF_8));
        sendThread.start();
    }

    public static void loadConfig() {
        Properties conf = new Properties();
        try (FileInputStream fileInputStream = new FileInputStream("conf/imgBridge.properties")) {
            conf.load(fileInputStream);
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
        picConf = new PicConf();
        picConf.setWidth(Integer.parseInt(conf.getProperty("width")));
        picConf.setHeight(Integer.parseInt(conf.getProperty("height")));
        picConf.setCellSize(Integer.parseInt(conf.getProperty("cellSize")));
        picConf.setWaterMask(Boolean.parseBoolean(conf.getProperty("waterMask")));
        if (picConf.getCellSize() < 4 || picConf.getCellSize() % 2 != 0) {
            if (picConf.isWaterMask()) {
                throw new RuntimeException("有水印时格子大小不能小于4且需要为偶数");
            }
        }
        picConf.setInterval(Long.parseLong(conf.getProperty("interval")));
        image = new BufferedImage(picConf.getWidth(), picConf.getHeight(), BufferedImage.TYPE_INT_RGB);
    }
}
