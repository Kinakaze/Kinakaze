import org.lwjgl.glfw.GLFWErrorCallback;
import org.lwjgl.opengl.GL;
import org.lwjgl.system.MemoryStack;

import java.nio.FloatBuffer;
import java.nio.IntBuffer;

import static org.lwjgl.glfw.GLFW.*;
import static org.lwjgl.opengl.GL32C.*;
import static org.lwjgl.system.MemoryStack.stackPush;

public final class LwjglSmoke {
    private static int shader(int type, String source) {
        int shader = glCreateShader(type);
        glShaderSource(shader, source);
        glCompileShader(shader);
        if (glGetShaderi(shader, GL_COMPILE_STATUS) == GL_FALSE) {
            throw new IllegalStateException(glGetShaderInfoLog(shader));
        }
        return shader;
    }

    public static void main(String[] args) {
        GLFWErrorCallback.createPrint(System.err).set();
        if (!glfwInit()) {
            throw new IllegalStateException("glfwInit failed");
        }
        glfwDefaultWindowHints();
        glfwWindowHint(GLFW_CONTEXT_VERSION_MAJOR, 3);
        glfwWindowHint(GLFW_CONTEXT_VERSION_MINOR, 2);
        glfwWindowHint(GLFW_OPENGL_PROFILE, GLFW_OPENGL_CORE_PROFILE);
        glfwWindowHint(GLFW_OPENGL_FORWARD_COMPAT, GLFW_TRUE);
        glfwWindowHint(GLFW_VISIBLE, GLFW_FALSE);
        glfwWindowHint(GLFW_RESIZABLE, GLFW_TRUE);
        long window = glfwCreateWindow(640, 400, "Kinakaze LWJGL core 3.2", 0, 0);
        if (window == 0) {
            throw new IllegalStateException("glfwCreateWindow failed");
        }
        glfwMakeContextCurrent(window);
        GL.createCapabilities();
        System.out.println("GL_VERSION=" + glGetString(GL_VERSION));
        System.out.println("GL_VENDOR=" + glGetString(GL_VENDOR));
        System.out.println("GL_RENDERER=" + glGetString(GL_RENDERER));

        int vertex = shader(GL_VERTEX_SHADER, """
            #version 150 core
            in vec2 position;
            in vec3 color;
            out vec3 vertexColor;
            void main() {
                vertexColor = color;
                gl_Position = vec4(position, 0.0, 1.0);
            }
            """);
        int fragment = shader(GL_FRAGMENT_SHADER, """
            #version 150 core
            in vec3 vertexColor;
            out vec4 outputColor;
            void main() { outputColor = vec4(vertexColor, 1.0); }
            """);
        int program = glCreateProgram();
        glAttachShader(program, vertex);
        glAttachShader(program, fragment);
        glBindAttribLocation(program, 0, "position");
        glBindAttribLocation(program, 1, "color");
        glLinkProgram(program);
        if (glGetProgrami(program, GL_LINK_STATUS) == GL_FALSE) {
            throw new IllegalStateException(glGetProgramInfoLog(program));
        }

        int vao = glGenVertexArrays();
        int vbo = glGenBuffers();
        glBindVertexArray(vao);
        glBindBuffer(GL_ARRAY_BUFFER, vbo);
        float[] vertices = {
             0.0f,  0.8f, 1.0f, 0.1f, 0.1f,
            -0.8f, -0.7f, 0.1f, 1.0f, 0.1f,
             0.8f, -0.7f, 0.1f, 0.2f, 1.0f
        };
        try (MemoryStack stack = stackPush()) {
            FloatBuffer data = stack.mallocFloat(vertices.length);
            data.put(vertices).flip();
            glBufferData(GL_ARRAY_BUFFER, data, GL_STATIC_DRAW);
        }
        glVertexAttribPointer(0, 2, GL_FLOAT, false, 5 * Float.BYTES, 0L);
        glVertexAttribPointer(1, 3, GL_FLOAT, false, 5 * Float.BYTES, 2L * Float.BYTES);
        glEnableVertexAttribArray(0);
        glEnableVertexAttribArray(1);
        glUseProgram(program);
        glfwSwapInterval(1);
        glfwShowWindow(window);

        long deadline = System.nanoTime() + 3_000_000_000L;
        while (!glfwWindowShouldClose(window) && System.nanoTime() < deadline) {
            try (MemoryStack stack = stackPush()) {
                IntBuffer width = stack.mallocInt(1);
                IntBuffer height = stack.mallocInt(1);
                glfwGetFramebufferSize(window, width, height);
                glViewport(0, 0, width.get(0), height.get(0));
            }
            glClearColor(0.04f, 0.06f, 0.12f, 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
            glDrawArrays(GL_TRIANGLES, 0, 3);
            glfwSwapBuffers(window);
            glfwPollEvents();
        }

        glDeleteBuffers(vbo);
        glDeleteVertexArrays(vao);
        glDeleteProgram(program);
        glDeleteShader(vertex);
        glDeleteShader(fragment);
        glfwDestroyWindow(window);
        glfwTerminate();
        System.out.println("LWJGL_SMOKE_OK");
    }
}
