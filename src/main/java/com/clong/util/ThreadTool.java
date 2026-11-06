package com.clong.util;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class ThreadTool {
    private static ExecutorService executorService = Executors.newSingleThreadExecutor();

    public static void execute(Runnable runnable){
        executorService.execute(runnable);
    }
}
