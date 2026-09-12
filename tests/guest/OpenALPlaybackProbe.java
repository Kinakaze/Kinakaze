import java.nio.ByteBuffer;
import java.nio.IntBuffer;
import java.nio.ShortBuffer;
import org.lwjgl.BufferUtils;
import org.lwjgl.openal.AL;
import org.lwjgl.openal.AL10;
import org.lwjgl.openal.AL11;
import org.lwjgl.openal.ALC;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALCCapabilities;

/** A short, low-volume real OpenAL playback probe for the Linux Java guest. */
public final class OpenALPlaybackProbe {
    private static void check(String operation) {
        int error = AL10.alGetError();
        if (error != AL10.AL_NO_ERROR) throw new IllegalStateException(operation + " AL error=" + error);
    }

    public static void main(String[] args) throws Exception {
        System.out.println("AUDIO_JAVA " + System.getProperty("java.version") + " " + System.getProperty("os.name"));
        long device = ALC10.alcOpenDevice((ByteBuffer) null);
        if (device == 0) throw new IllegalStateException("alcOpenDevice failed");
        long context = 0;
        int buffer = 0, source = 0;
        try {
            ALCCapabilities capabilities = ALC.createCapabilities(device);
            context = ALC10.alcCreateContext(device, (IntBuffer) null);
            if (context == 0 || !ALC10.alcMakeContextCurrent(context)) throw new IllegalStateException("alcCreateContext failed");
            AL.createCapabilities(capabilities);
            System.out.println("AUDIO_DEVICE " + ALC10.alcGetString(device, ALC10.ALC_DEVICE_SPECIFIER));
            System.out.println("AUDIO_RENDERER " + AL10.alGetString(AL10.AL_RENDERER));
            int rate = 48_000;
            int frameCount = rate * 3;
            ShortBuffer samples = BufferUtils.createShortBuffer(frameCount);
            for (int index = 0; index < frameCount; index++) {
                double fade = Math.min(1.0, Math.min(index, frameCount - 1 - index) / 480.0);
                samples.put((short) (Math.sin(2.0 * Math.PI * 440.0 * index / rate) * 2_000 * fade));
            }
            samples.flip();
            buffer = AL10.alGenBuffers();
            source = AL10.alGenSources();
            AL10.alBufferData(buffer, AL10.AL_FORMAT_MONO16, samples, rate);
            AL10.alSourcei(source, AL10.AL_BUFFER, buffer);
            AL10.alSourcef(source, AL10.AL_GAIN, 0.5f);
            check("buffer upload");
            System.out.println("AUDIO_READY rate=" + rate + " frames=" + frameCount + " tone=440 gain=0.5 amplitude=2000");
            System.out.flush();
            Thread.sleep(1500);
            AL10.alSourcePlay(source);
            check("play");
            boolean progressed = false;
            int previous = -1;
            for (int index = 0; index < 12; index++) {
                Thread.sleep(200);
                int state = AL10.alGetSourcei(source, AL10.AL_SOURCE_STATE);
                int offset = AL10.alGetSourcei(source, AL11.AL_SAMPLE_OFFSET);
                check("progress query");
                System.out.println("AUDIO_PROGRESS state=" + state + " offset=" + offset);
                if (state != AL10.AL_PLAYING) throw new IllegalStateException("source stopped before the submitted duration");
                progressed |= previous >= 0 && offset > previous;
                previous = offset;
            }
            if (!progressed) throw new IllegalStateException("source sample offset never advanced");
            Thread.sleep(900);
            if (AL10.alGetSourcei(source, AL10.AL_SOURCE_STATE) != AL10.AL_STOPPED) throw new IllegalStateException("source did not finish");
            check("finish");
            System.out.println("AUDIO_PLAYBACK_COMPLETE frames=" + frameCount);
        } finally {
            if (source != 0) AL10.alDeleteSources(source);
            if (buffer != 0) AL10.alDeleteBuffers(buffer);
            ALC10.alcMakeContextCurrent(0);
            if (context != 0) ALC10.alcDestroyContext(context);
            if (!ALC10.alcCloseDevice(device)) throw new IllegalStateException("alcCloseDevice failed");
        }
    }
}
