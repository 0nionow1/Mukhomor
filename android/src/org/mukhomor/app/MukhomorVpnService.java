package org.mukhomor.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.net.VpnService;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import org.json.JSONArray;
import org.json.JSONObject;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class MukhomorVpnService extends VpnService {
    private static final String CHANNEL="vpn";
    private static final ExecutorService worker=Executors.newSingleThreadExecutor();
    private volatile ParcelFileDescriptor tunnel;
    private static final ConcurrentLinkedQueue<Job> jobs=new ConcurrentLinkedQueue<>();
    private static final class Job {
        final String request; final long generation; final CompletableFuture<String> result=new CompletableFuture<>();
        Job(String r,long g){request=r;generation=g;}
    }
    static CompletableFuture<String> enqueue(String request,long generation) { Job j=new Job(request,generation);jobs.add(j);return j.result; }
    @Override public void onCreate() {
        super.onCreate();NativeBridge.context=getApplicationContext();NativeBridge.service=this;
        NotificationManager manager=getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(CHANNEL,"VPN",NotificationManager.IMPORTANCE_LOW));
    }
    private Notification notification(String text) {
        PendingIntent open=PendingIntent.getActivity(this,0,new Intent(this,MainActivity.class),PendingIntent.FLAG_IMMUTABLE|PendingIntent.FLAG_UPDATE_CURRENT);
        PendingIntent stop=PendingIntent.getService(this,1,new Intent(this,MukhomorVpnService.class).setAction("DISCONNECT"),PendingIntent.FLAG_IMMUTABLE|PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(this,CHANNEL).setContentTitle("Mukhomor").setContentText(text)
            .setSmallIcon(org.mukhomor.app.R.drawable.ic_stat_mushroom).setContentIntent(open).setOngoing(true)
            .addAction(new Notification.Action.Builder(null,"Отключиться",stop).build()).build();
    }
    @Override public int onStartCommand(Intent intent,int flags,int startId) {
        startForeground(800,notification("Подключение VPN…"));
        if(intent!=null&&"DISCONNECT".equals(intent.getAction())){NativeBridge.generation.incrementAndGet();worker.execute(()->{NativeBridge.invoke("{\"action\":\"Disconnect\"}",-1);stopVpn();});return START_NOT_STICKY;}
        if(jobs.isEmpty()){
            // Android always-on and process restart restore the last successful
            // server through the same consent and controller path.
            String status=NativeBridge.invoke("{\"action\":\"Status\"}",-1);
            try{JSONObject data=new JSONObject(status).getJSONObject("data");String last=data.optString("last_connected");if(!last.isEmpty())NativeBridge.invoke(new JSONObject().put("action","Select").put("id",last).toString(),-1);}catch(Exception ignored){}
            enqueue("{\"action\":\"Connect\",\"request_id\":\"android-system-"+System.nanoTime()+"\"}",NativeBridge.generation.incrementAndGet());
        }
        worker.execute(()->{Job job;while((job=jobs.poll())!=null)connect(job);});
        return START_STICKY;
    }
    private void connect(Job job) {
        if(job.generation!=NativeBridge.generation.get()){job.result.complete(NativeBridge.failure(job.request,"Connection cancelled"));return;}
        try{
            if(tunnel!=null){job.result.complete(NativeBridge.invoke(job.request,-1));return;}
            if(VpnService.prepare(this)!=null)throw new IllegalStateException("VPN permission is missing");
            Builder builder=new Builder().setSession("Mukhomor").setMtu(1500)
                // Mihomo derives its IPv4 TUN address from dns.fake-ip-range.
                .addAddress("198.18.0.1",30).addAddress("fdfe:dcba:9876::1",126)
                .addRoute("0.0.0.0",0).addRoute("::",0).addDnsServer("198.18.0.2")
                .setConfigureIntent(PendingIntent.getActivity(this,0,new Intent(this,MainActivity.class),PendingIntent.FLAG_IMMUTABLE));
            if(Build.VERSION.SDK_INT>=29)builder.setMetered(false);
            builder.addDisallowedApplication(getPackageName());
            JSONArray packages=new JSONArray(new String(NativeBridge.nativePackages(),StandardCharsets.UTF_8));
            for(int i=0;i<packages.length();i++){try{builder.addDisallowedApplication(packages.getString(i));}catch(android.content.pm.PackageManager.NameNotFoundException ignored){}}
            tunnel=builder.establish();if(tunnel==null)throw new IllegalStateException("VPN permission was revoked");
            // The bundled TUN factory duplicates this borrowed FD exactly when
            // ownership passes to Go; failed parsing never leaks a descriptor.
            int fd=tunnel.getFd();
            String response=NativeBridge.invoke(job.request,fd);
            if(!new JSONObject(response).optBoolean("ok")||job.generation!=NativeBridge.generation.get()){
                NativeBridge.invoke("{\"action\":\"Disconnect\"}",-1);closeTunnel();stopSelf();
                if(job.generation!=NativeBridge.generation.get())response=NativeBridge.failure(job.request,"Connection cancelled");
            }else {
                getSystemService(NotificationManager.class).notify(800,notification("VPN подключён"));
                NativeBridge.requestNotificationPermission();
            }
            job.result.complete(response);
        }catch(Exception e){NativeBridge.invoke("{\"action\":\"Disconnect\"}",-1);closeTunnel();stopSelf();job.result.complete(NativeBridge.failure(job.request,"Cannot establish Android VPN"));}
    }
    private synchronized void closeTunnel(){if(tunnel!=null){try{tunnel.close();}catch(Exception ignored){}tunnel=null;}}
    void stopVpn(){closeTunnel();stopForeground(STOP_FOREGROUND_REMOVE);stopSelf();}
    @Override public void onRevoke(){
        NativeBridge.generation.incrementAndGet();closeTunnel();
        worker.execute(()->{NativeBridge.invoke("{\"action\":\"Disconnect\"}",-1);stopVpn();});
        super.onRevoke();
    }
    @Override public void onDestroy(){
        if(NativeBridge.service==this){NativeBridge.generation.incrementAndGet();NativeBridge.service=null;}
        closeTunnel();
        // Native shutdown can wait for an in-flight connect. Never block the
        // Android main thread or stop a replacement service started meanwhile.
        worker.execute(()->{if(NativeBridge.service==null)NativeBridge.invoke("{\"action\":\"Disconnect\"}",-1);});
        super.onDestroy();
    }
}
