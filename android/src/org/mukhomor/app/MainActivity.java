package org.mukhomor.app;

import android.app.NativeActivity;
import android.app.Dialog;
import android.animation.ValueAnimator;
import android.content.Intent;
import android.graphics.drawable.Animatable;
import android.graphics.drawable.ColorDrawable;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.view.animation.DecelerateInterpolator;
import android.widget.FrameLayout;
import android.widget.ImageView;

public final class MainActivity extends NativeActivity {
    private LaunchOverlay launchOverlay;

    public Class<?> bridgeClass() { return NativeBridge.class; }
    @Override public void onCreate(Bundle state) {
        NativeBridge.attach(this);
        super.onCreate(state);
        if (Build.VERSION.SDK_INT >= 31) {
            // NativeActivity takes ownership of the main surface before the
            // framework exit callback. Prepare this attached surface first.
            launchOverlay = new LaunchOverlay(this);
            launchOverlay.show();
            getSplashScreen().setOnExitAnimationListener(splash -> splash.remove());
        }
    }
    void firstFrame() {
        if (launchOverlay != null) launchOverlay.reveal();
    }
    private static final class LaunchOverlay extends Dialog {
        private final FrameLayout scene;
        private final ImageView mushroom;
        private ValueAnimator fade;

        LaunchOverlay(MainActivity owner) {
            super(owner, android.R.style.Theme_Material_NoActionBar);
            setCancelable(false);
            scene = new FrameLayout(owner);
            scene.setBackgroundColor(owner.getColor(R.color.launcher_background));
            mushroom = place(owner, R.drawable.startup_mushroom_holding, false);
            place(owner, R.drawable.startup_wordmark, true);
            scene.addOnLayoutChangeListener((view, l, t, r, b, ol, ot, or, ob) -> {
                int[] origin = new int[2];
                scene.getLocationOnScreen(origin);
                for (int i=0; i<scene.getChildCount(); i++) {
                    View child = scene.getChildAt(i);
                    int[] position = (int[])child.getTag();
                    FrameLayout.LayoutParams layout = (FrameLayout.LayoutParams)child.getLayoutParams();
                    int x=position[0]-origin[0], y=position[1]-origin[1];
                    if (layout.leftMargin != x || layout.topMargin != y) {
                        layout.leftMargin=x; layout.topMargin=y; child.setLayoutParams(layout);
                    }
                }
            });
            setContentView(scene);
            Window window = getWindow();
            window.setBackgroundDrawable(new ColorDrawable(owner.getColor(R.color.launcher_background)));
            window.clearFlags(WindowManager.LayoutParams.FLAG_DIM_BEHIND);
            window.addFlags(WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE);
            window.setDecorFitsSystemWindows(false);
            window.setStatusBarColor(owner.getColor(R.color.launcher_background));
            window.setNavigationBarColor(owner.getColor(R.color.launcher_background));
        }
        private ImageView place(MainActivity owner, int resource, boolean bottom) {
            float density = owner.getResources().getDisplayMetrics().density;
            int width = Math.round((bottom ? 200 : 288) * density);
            int height = Math.round((bottom ? 80 : 288) * density);
            int[] position = new int[2];
            android.graphics.Rect bounds = owner.getWindowManager().getCurrentWindowMetrics().getBounds();
            position[0] = (bounds.width()-width)/2;
            position[1] = bottom ? bounds.height()-height-Math.round(56*density) : (bounds.height()-height)/2;
            ImageView image = new ImageView(owner);
            image.setImageResource(resource);
            image.setTag(position);
            FrameLayout.LayoutParams layout = new FrameLayout.LayoutParams(width, height);
            layout.leftMargin = position[0]; layout.topMargin = position[1];
            scene.addView(image, layout);
            return image;
        }
        @Override public void show() {
            super.show();
            getWindow().setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
            if (ValueAnimator.areAnimatorsEnabled()) ((Animatable)mushroom.getDrawable()).start();
        }
        void reveal() {
            if (!isShowing() || fade != null) return;
            if (!ValueAnimator.areAnimatorsEnabled()) { dismiss(); return; }
            fade = ValueAnimator.ofFloat(1f, 0f);
            fade.setDuration(220);
            fade.setInterpolator(new DecelerateInterpolator());
            fade.addUpdateListener(animation -> {
                WindowManager.LayoutParams attributes = getWindow().getAttributes();
                attributes.alpha = (float)animation.getAnimatedValue();
                getWindow().setAttributes(attributes);
            });
            fade.addListener(new android.animation.AnimatorListenerAdapter() {
                @Override public void onAnimationEnd(android.animation.Animator animation) { dismiss(); }
            });
            fade.start();
        }
        @Override public void dismiss() {
            if (fade != null) { fade.removeAllListeners(); fade.cancel(); }
            ((Animatable)mushroom.getDrawable()).stop();
            super.dismiss();
        }
    }
    @Override protected void onResume() { super.onResume(); NativeBridge.attach(this); }
    @Override protected void onDestroy() {
        if (launchOverlay != null) { launchOverlay.dismiss(); launchOverlay = null; }
        NativeBridge.detach(this);
        super.onDestroy();
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        if (!NativeBridge.activityResult(request, result, data)) super.onActivityResult(request, result, data);
    }
}
