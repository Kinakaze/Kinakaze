import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.concurrent.CompletableFuture;

/** Exercises the actual Linux JVM, JIT, threads, file I/O and process handoff. */
public final class JavaRuntimeProbe {
    private static long sum(int count) {
        long result = 0;
        for (int i = 0; i < count; i++) result += (long) i * i;
        return result;
    }

    public static void main(String[] args) throws Exception {
        if (!System.getProperty("os.name").equals("Linux")) throw new AssertionError("not guest JVM");
        System.out.println("JAVA_MAIN_ENTERED");
        long expected = 333328333350000L;
        for (int i = 0; i < 300; i++) if (sum(100000) != expected) throw new AssertionError("JIT result");
        var tasks = new ArrayList<CompletableFuture<Long>>();
        for (int i = 0; i < 8; i++) tasks.add(CompletableFuture.supplyAsync(() -> sum(100000)));
        for (var task : tasks) if (task.get() != expected) throw new AssertionError("thread result");
        Path file = Files.createTempFile("kinakaze-java-", ".txt");
        try {
            Files.writeString(file, "Java UTF-8: \u4e2d\u6587\n", StandardOpenOption.TRUNCATE_EXISTING);
            if (!Files.readString(file).equals("Java UTF-8: \u4e2d\u6587\n")) throw new AssertionError("file contents");
            if (!Files.isSameFile(file, file.toRealPath())) throw new AssertionError("realpath identity");
        } finally { Files.delete(file); }
        System.out.println("JAVA_JIT_THREADS_FILES_OK");
        if (args.length > 0 && args[0].equals("spawn")) {
            for (int i = 0; i < 8; i++) {
                Process child = new ProcessBuilder("/bin/sh", "-c", "printf stdout; printf stderr >&2; exit 7")
                        .redirectErrorStream(true).start();
                String output = new String(child.getInputStream().readAllBytes(), java.nio.charset.StandardCharsets.UTF_8);
                if (child.waitFor() != 7 || !output.equals("stdoutstderr")) throw new AssertionError("spawn result");
            }
            System.out.println("JAVA_SPAWN_OK");
        }
    }
}
