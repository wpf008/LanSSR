import com.clong.PicReceiver;
import com.clong.PicTransmitter;
import com.clong.data.PicConf;
import com.clong.thread.RPaserThread;
import com.clong.thread.SendThread;
import com.clong.util.ImgUtil;
import com.clong.view.PicPanel;

import java.awt.Graphics2D;
import java.awt.Rectangle;
import java.awt.image.BufferedImage;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import javax.imageio.ImageIO;

/**
 * Interoperability fixture. Encoding and decoding are performed by the
 * unmodified Java implementation; this harness only supplies a headless grid
 * and replaces screen painting with PNG capture.
 */
public final class Compat {
    private static final int WIDTH = 128;
    private static final int HEIGHT = 128;
    private static final int CELL = 4;

    public static void main(String[] args) throws Exception {
        boolean watermark = Boolean.parseBoolean(args[1]);
        Path directory = Paths.get(args[2]);
        setup(watermark);
        if (args[0].equals("encode")) {
            PicTransmitter.panel = new CapturePanel(directory);
            SendThread sender = new SendThread();
            if (args[4].equals("text")) {
                sender.setTextData(Files.readAllBytes(Paths.get(args[3])));
            } else {
                sender.setFilePath(args[3]);
            }
            sender.run();
        } else if (args[0].equals("decode")) {
            for (int i = 0; i < 2; i++) {
                BufferedImage image = ImageIO.read(directory.resolve("frame-" + i + ".png").toFile());
                byte[] bytes = ImgUtil.parseBytes(PicTransmitter.cellInfoList, image);
                Files.write(directory.resolve("frame-" + i + ".bin"), bytes);
                RPaserThread parser = new RPaserThread();
                parser.setScreen(image);
                parser.run();
            }
            String metadata = (PicReceiver.isText ? "text" : "file") + "\n"
                    + PicReceiver.total + "\n" + PicReceiver.fileName;
            Files.write(directory.resolve("metadata.txt"), metadata.getBytes(StandardCharsets.UTF_8));
            byte[] payload = PicReceiver.dataMap.get(1);
            if (payload == null) {
                throw new AssertionError("Original Java receiver rejected the Rust payload");
            }
            Files.write(directory.resolve("received.bin"), payload);
        } else {
            throw new IllegalArgumentException("Unknown operation: " + args[0]);
        }
    }

    private static void setup(boolean watermark) {
        PicConf config = new PicConf();
        config.setWidth(WIDTH);
        config.setHeight(HEIGHT);
        config.setCellSize(CELL);
        config.setWaterMask(watermark);
        config.setInterval(0);
        PicTransmitter.picConf = config;
        PicTransmitter.image = new BufferedImage(WIDTH, HEIGHT, BufferedImage.TYPE_INT_RGB);
        PicTransmitter.isStop = false;
        PicTransmitter.cellInfoList.clear();
        PicTransmitter.lineField.setText("");
        for (int y = 0; y < HEIGHT; y += CELL) {
            for (int x = 0; x < WIDTH; x += CELL) {
                PicTransmitter.cellInfoList.add(new Rectangle(x, y, CELL, CELL));
            }
        }
        ImgUtil.waterMask = watermark;
    }

    private static final class CapturePanel extends PicPanel {
        private final Path directory;
        private int count;

        CapturePanel(Path directory) {
            this.directory = directory;
        }

        @Override
        public void repaint() {
            // Swing calls repaint in its constructor, before directory is set.
            if (directory == null || PicTransmitter.image == null) {
                return;
            }
            try {
                BufferedImage copy = new BufferedImage(Compat.WIDTH, Compat.HEIGHT, BufferedImage.TYPE_INT_RGB);
                Graphics2D graphics = copy.createGraphics();
                graphics.drawImage(PicTransmitter.image, 0, 0, null);
                graphics.dispose();
                ImageIO.write(copy, "png", directory.resolve("frame-" + count++ + ".png").toFile());
            } catch (Exception error) {
                throw new IllegalStateException("Could not capture Java sender frame", error);
            }
        }
    }
}
