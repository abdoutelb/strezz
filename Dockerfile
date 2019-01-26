FROM ubuntu:16.04

RUN  apt-get update  && apt-get  install -y apt-transport-https \
 && apt-get  install -y curl \
 && apt-get  install -y python-pip python-dev build-essential \
 &&  pip install locustio 
# Bundle app source



COPY . .

EXPOSE 4000
# this is the build args parameter should be the same name
ARG url    
ENV SITE_URL=${url}


# this should be always simple string
CMD locust -f /loc.py --host=$SITE_URL -P 4000


#TO buld the images  docker build -t stress --build-arg url=http://www.akhbarak.net/ .
#TO run the container docker run -itd -p 4000:4000 -label stress:latest