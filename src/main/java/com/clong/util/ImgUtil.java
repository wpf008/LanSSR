package com.clong.util;

import com.clong.data.Position;
import com.clong.data.Range;

import java.awt.*;
import java.awt.image.BufferedImage;
import java.util.List;
import java.util.*;

public class ImgUtil {
    public static Robot robot = null;

    public static Rectangle defRect;

    public static boolean waterMask = false;

    public static Position findPosition() {
        // 获取有几个屏幕
        GraphicsDevice[] screenDevices = GraphicsEnvironment.getLocalGraphicsEnvironment().getScreenDevices();
        int num = 0;
        for (GraphicsDevice screen : screenDevices) {
            Rectangle bounds = screen.getDefaultConfiguration().getBounds();
            System.out.println("屏幕扫描：" + num + "," + bounds);
            Position pos = findInScreen(bounds);
            if (pos != null) {
                Rectangle rectangle = pos.getRectangle();
                if (rectangle.width % pos.getCellSize() != 0 || rectangle.height % pos.getCellSize() != 0) {
                    System.out.println("发现异常的内容界面：" + pos);
                    continue;
                }
                defRect = bounds;
                return pos;
            }
            num++;
        }
        return null;
    }

    private static Position findInScreen(Rectangle bounds) {
        BufferedImage screenCapture = getScreenCapture(bounds);

        // 第一次扫图，找出格子大小
        Map<Integer, Integer> distanceMap = new HashMap<>();
        for (int y = 0; y < screenCapture.getHeight(); y++) {
            byte last = -1;
            int lastX = -1;
            for (int x = 0; x < screenCapture.getWidth(); x++) {
                int rgb = screenCapture.getRGB(x, y);
                Color color = new Color(rgb);
                int gray = getGray(color);
                byte cur = 0;
                if (gray < 128) {
                    cur = 1;
                }
                if (last != -1 && last != cur) {
                    if (lastX > -1) {
                        Integer distance = x - lastX;
                        if (distanceMap.containsKey(distance)) {
                            distanceMap.put(distance, distanceMap.get(distance) + 1);
                        } else {
                            distanceMap.put(distance, 1);
                        }
                    }
                    lastX = x;
                }
                last = cur;
            }
        }
        Iterator<Map.Entry<Integer, Integer>> iterator = distanceMap.entrySet().iterator();
        int maxDistance = -1;
        int maxCount = 0;
        while (iterator.hasNext()) {
            Map.Entry<Integer, Integer> next = iterator.next();
            if (next.getKey() == 1) {
                continue;
            }
            if (next.getValue() > maxCount) {
                maxDistance = next.getKey();
                maxCount = next.getValue();
            }
        }
        if (maxCount < 20000) {
            // 没找到格子
            return null;
        }

        int minX = -1;
        int maxX = -1;
        int minY = -1;
        int maxY = -1;
        // 第二次扫图，确定起始位置和宽高
        for (int y = 0; y < screenCapture.getHeight(); y++) {
            byte last = -1;
            int lastX = -1;
            List<Integer> changePoints = new ArrayList<>();
            for (int x = 0; x < screenCapture.getWidth(); x++) {
                int rgb = screenCapture.getRGB(x, y);
                Color color = new Color(rgb);
                int gray = getGray(color);
                byte cur = 0;
                if (gray < 128) {
                    cur = 1;
                }
                if (last != -1 && last != cur) {
                    if (lastX > -1) {
                        Integer distance = x - lastX;
                        if (distanceMap.containsKey(distance)) {
                            distanceMap.put(distance, distanceMap.get(distance) + 1);
                        } else {
                            distanceMap.put(distance, 1);
                        }
                    }
                    lastX = x;
                    changePoints.add(x);
                }
                last = cur;
            }
            Range range = anylize(changePoints, maxDistance);
            if (range != null) {
                if (maxX == -1) {
                    minX = range.getStart();
                } else {
                    minX = Math.min(minX, range.getStart());
                }
                maxX = Math.max(maxX, range.getEnd());
            }

            if (minY == -1) {
                if (range != null) {
                    minY = y;
                }
            } else {
                if (range == null) {
                    maxY = y;
                    break;
                }
            }
        }
        if (minY == -1 || minX == -1 || maxX == -1 || maxY == -1) {
            return null;
        }
        Position position = new Position();
        position.setCellSize(maxDistance);
        Rectangle rectangle = new Rectangle();
        rectangle.x = minX + bounds.x;
        rectangle.y = minY + bounds.y;
        rectangle.width = maxX - minX;
        rectangle.height = maxY - minY;
        position.setRectangle(rectangle);
        return position;
    }

    public static BufferedImage getScreenCapture(Rectangle bounds) {
        if (robot == null) {
            try {
                robot = new Robot();
            } catch (AWTException e) {
                throw new RuntimeException(e);
            }
        }

        return robot.createScreenCapture(bounds);
    }

    private static Range anylize(List<Integer> changePoints, int maxDistance) {
        if (changePoints.size() < 30) {
            return null;
        }
        int count = 0;
        int start = -1;
        int end = -1;
        for (int i = 0; i < changePoints.size() - 1; i++) {
            int i1 = changePoints.get(i + 1) - changePoints.get(i);
            if (i1 == maxDistance) {
                if (start == -1) {
                    start = changePoints.get(i);
                }
                count++;
            } else {
                if (count > 30) {
                    end = changePoints.get(i);
                    break;
                } else {
                    start = -1;
                    count = 0;
                }
            }
        }

        if (end == -1) {
            return null;
        }
        return new Range(start, end);
    }

    public static int getGray(Color color) {
        int r = color.getRed();
        int g = color.getGreen();
        int b = color.getBlue();
        if (r == g && g == b) {
            return r;
        }

        return (int) (0.299D * r + 0.587D * g + 0.114D * b);
    }

    /**
     * 从格子解析为字节数组（二进制编码版本）
     * 每两个格子组成一个字节
     */
    public static byte[] parseBytes(List<Rectangle> cells, BufferedImage screen) {
        int byteCount = cells.size() / 2;
        byte[] result = new byte[byteCount];
        
        for (int i = 0; i < byteCount; i++) {
            int high = parseCell4Bit(cells.get(i * 2), screen);
            int low = parseCell4Bit(cells.get(i * 2 + 1), screen);
            if (high < 0 || low < 0) {
                // 遇到红色标记，返回已解析的部分
                return Arrays.copyOf(result, i);
            }
            result[i] = (byte) ((high << 4) | low);
        }
        return result;
    }

    public static boolean isWaterMaskMode(){
        return waterMask;
    }

    /**
     * 解析单个格子的 4 bit 值
     */
    private static int parseCell4Bit(Rectangle rect, BufferedImage screen) {
        // 从静态变量或参数中获取水印模式标志
        // 这里我们通过检查PicReceiver或PicTransmitter的静态配置来判断
        boolean waterMaskMode = isWaterMaskMode();

        if (waterMaskMode) {
            // 水印模式：解析4个小格子的黑白状态
            int halfWidth = rect.width / 2;
            int halfHeight = rect.height / 2;

            // 定义4个小格子的位置
            Rectangle[] subCells = new Rectangle[4];
            subCells[0] = new Rectangle(rect.x, rect.y, halfWidth, halfHeight);                    // 左上
            subCells[1] = new Rectangle(rect.x + halfWidth, rect.y, halfWidth, halfHeight);       // 右上
            subCells[2] = new Rectangle(rect.x, rect.y + halfHeight, halfWidth, halfHeight);      // 左下
            subCells[3] = new Rectangle(rect.x + halfWidth, rect.y + halfHeight, halfWidth, halfHeight); // 右下

            int value = 0;
            for (int i = 0; i < 4; i++) {
                int gray = getAverageGray(subCells[i], screen);
                if (gray < 0) {
                    // 检测到红色标记
                    return -1;
                }
                // 灰度<128判定为黑色(1)，否则为白色(0)
                if (gray < 128) {
                    value |= (1 << (3 - i));  // 设置对应的位
                }
            }
            return value;
        } else {
            // 无水印模式：使用灰度值解析
            int sum = 0;
            int count = 0;
            for (int x = rect.x; x < rect.x + rect.width; x++) {
                for (int y = rect.y; y < rect.y + rect.height; y++) {
                    int rgb = screen.getRGB(x, y);
                    Color color = new Color(rgb);
                    // 检测红色标记（帧结束）
                    if (color.getRed() > 200 && color.getGreen() < 50 && color.getBlue() < 50) {
                        return -1;
                    }
                    sum += getGray(color);
                    count++;
                }
            }
            int gray = sum / count;
            // 按16级灰度区间判定：0-15为0，16-31为1，依次类推
            int value = gray / 16;
            if (value > 15) value = 15;
            if (value < 0) value = 0;
            return value;
        }
    }

    /**
     * 计算指定区域的平均灰度值
     * @return 平均灰度值，如果检测到红色标记则返回-1
     */
    private static int getAverageGray(Rectangle rect, BufferedImage screen) {
        int sum = 0;
        int count = 0;
        for (int x = rect.x; x < rect.x + rect.width; x++) {
            for (int y = rect.y; y < rect.y + rect.height; y++) {
                int rgb = screen.getRGB(x, y);
                Color color = new Color(rgb);
                // 检测红色标记
                if (color.getRed() > 200 && color.getGreen() < 50 && color.getBlue() < 50) {
                    return -1;
                }
                sum += getGray(color);
                count++;
            }
        }
        return count > 0 ? sum / count : 128;
    }

    public static String simpleMsg(List<Rectangle> cells, BufferedImage screen) {
        StringBuilder sb = new StringBuilder();
        // 只取前 20 个格子用于判断帧变化
        for (int i = 0; i < Math.min(20, cells.size()); i++) {
            String v = cellValue(cells.get(i), screen);
            if (v == null) {
                return sb.toString();
            }
            sb.append(v);
        }
        return sb.toString();
    }

    public static String cellValue(Rectangle rect, BufferedImage screen) {
        int value = parseCell4Bit(rect, screen);
        if (value < 0) {
            return null;
        }
        return Integer.toHexString(value);
    }
}
